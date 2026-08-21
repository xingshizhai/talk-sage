//! 会话级慢插件执行器。
//!
//! observer 的 `run` 可能访问 LLM/知识库，不能占用实时音频线程。这里使用
//! 固定 worker 数和有界内存队列，替代“每个 segment/plugin spawn 一个线程”。

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use talksage_core::TranscriptSegment;
use talksage_plugins::{PluginContext, SegmentObserver};

use crate::{EventSink, PLUGIN_RUN_TIMEOUT};

struct PluginJob {
    plugin: Arc<dyn SegmentObserver>,
    ctx: PluginContext,
    emit: EventSink,
    segment: TranscriptSegment,
}

struct QueueState {
    jobs: VecDeque<PluginJob>,
    accepting: bool,
}

struct Shared {
    state: Mutex<QueueState>,
    ready: Condvar,
    capacity: usize,
    active: AtomicBool,
    cancel: Arc<AtomicBool>,
}

/// 可克隆的提交端。`submit` 只持有一次很短的内存锁，不执行插件代码。
#[derive(Clone)]
pub(super) struct PluginExecutorHandle {
    shared: Arc<Shared>,
}

impl PluginExecutorHandle {
    /// 队列满时丢弃新的慢任务；同步 skeleton 已经发出，不影响实时主链路。
    pub(super) fn submit(
        &self,
        plugin: Arc<dyn SegmentObserver>,
        ctx: PluginContext,
        emit: EventSink,
        segment: TranscriptSegment,
    ) -> bool {
        let Ok(mut state) = self.shared.state.lock() else {
            log::warn!("插件执行器队列锁已损坏，丢弃插件任务");
            return false;
        };
        if !state.accepting || state.jobs.len() >= self.shared.capacity {
            log::warn!(
                "插件[{}] 队列已满或会话已停止，丢弃慢任务（capacity={}）",
                plugin.name(),
                self.shared.capacity
            );
            return false;
        }
        state.jobs.push_back(PluginJob {
            plugin,
            ctx,
            emit,
            segment,
        });
        self.shared.ready.notify_one();
        true
    }
}

pub(super) struct PluginExecutor {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl PluginExecutor {
    pub(super) fn new(worker_count: usize, capacity: usize, cancel: Arc<AtomicBool>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(QueueState {
                jobs: VecDeque::new(),
                accepting: true,
            }),
            ready: Condvar::new(),
            capacity: capacity.max(1),
            active: AtomicBool::new(true),
            cancel,
        });
        let mut workers = Vec::with_capacity(worker_count.max(1));
        for index in 0..worker_count.max(1) {
            let worker_shared = shared.clone();
            match std::thread::Builder::new()
                .name(format!("plugin-worker-{index}"))
                .spawn(move || worker_loop(worker_shared))
            {
                Ok(worker) => workers.push(worker),
                Err(err) => log::warn!("启动插件 worker #{index} 失败: {err}"),
            }
        }
        Self { shared, workers }
    }

    pub(super) fn handle(&self) -> PluginExecutorHandle {
        PluginExecutorHandle {
            shared: self.shared.clone(),
        }
    }

    /// 停止接收。正常完成时允许队列在限定时间内 drain；取消时立即清队列。
    /// 运行中的外部调用无法强制中断，超时后其结果会被 active=false 丢弃。
    pub(super) fn shutdown(&mut self, graceful: bool, join_timeout: Duration) {
        if !graceful {
            self.shared.active.store(false, Ordering::Release);
        }
        if let Ok(mut state) = self.shared.state.lock() {
            state.accepting = false;
            if !graceful {
                state.jobs.clear();
            }
            self.shared.ready.notify_all();
        }
        let mut all_joined = true;
        for worker in self.workers.drain(..) {
            if !super::join_with_timeout(worker, join_timeout) {
                all_joined = false;
                log::warn!("插件 worker 停止超时，后台调用完成后将丢弃结果");
            }
        }
        if !all_joined || !graceful {
            self.shared.active.store(false, Ordering::Release);
        }
    }
}

impl Drop for PluginExecutor {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            self.shutdown(false, Duration::from_millis(100));
        }
    }
}

fn worker_loop(shared: Arc<Shared>) {
    loop {
        let job = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            loop {
                if let Some(job) = state.jobs.pop_front() {
                    break job;
                }
                if !state.accepting {
                    return;
                }
                let Ok(next) = shared.ready.wait(state) else {
                    return;
                };
                state = next;
            }
        };

        let name = job.plugin.name();
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| job.plugin.run(&job.segment, &job.ctx)));
        let elapsed = started.elapsed();
        if shared.cancel.load(Ordering::Relaxed) || !shared.active.load(Ordering::Acquire) {
            log::info!("插件[{name}] 会话已停止，丢弃结果");
            continue;
        }
        if elapsed > PLUGIN_RUN_TIMEOUT {
            log::warn!("插件[{name}] 超时 {elapsed:?}，丢弃结果");
            continue;
        }
        match result {
            Ok(Some(event)) => {
                log::info!("插件[{name}] 完成: 耗时={elapsed:?} 有结果=true");
                (job.emit)(event);
            }
            Ok(None) => log::info!("插件[{name}] 完成: 耗时={elapsed:?} 有结果=false"),
            Err(_) => log::warn!("插件[{name}] 执行 panic，任务已隔离"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use talksage_core::DomainEvent;

    struct BlockingObserver {
        started: mpsc::Sender<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl SegmentObserver for BlockingObserver {
        fn name(&self) -> &'static str {
            "blocking"
        }

        fn should_trigger(&self, _seg: &TranscriptSegment) -> bool {
            true
        }

        fn skeleton(&self, _seg: &TranscriptSegment) -> Vec<DomainEvent> {
            Vec::new()
        }

        fn run(&self, _seg: &TranscriptSegment, _ctx: &PluginContext) -> Option<DomainEvent> {
            let _ = self.started.send(());
            let (lock, ready) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = ready.wait(released).unwrap();
            }
            None
        }
    }

    fn segment(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: 0,
            speaker_label: "我".into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 100,
            rms: 0.1,
        }
    }

    #[test]
    fn bounded_queue_rejects_excess_jobs_without_blocking_submitter() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut executor = PluginExecutor::new(1, 1, cancel);
        let handle = executor.handle();
        let (started_tx, started_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let plugin = Arc::new(BlockingObserver {
            started: started_tx,
            release: release.clone(),
        });
        let emit: EventSink = Arc::new(|_| {});

        assert!(handle.submit(
            plugin.clone(),
            PluginContext::default(),
            emit.clone(),
            segment("一")
        ));
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(handle.submit(
            plugin.clone(),
            PluginContext::default(),
            emit.clone(),
            segment("二")
        ));
        assert!(!handle.submit(plugin, PluginContext::default(), emit, segment("三")));

        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();
        executor.shutdown(true, Duration::from_secs(1));
    }
}
