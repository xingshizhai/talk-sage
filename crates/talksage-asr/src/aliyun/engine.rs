//! 阿里云实时语音识别 WebSocket 引擎。

use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

use super::TokenManager;
use crate::EngineKind;

#[derive(Debug)]
enum AliyunEvent {
    Ready,
    Partial(String),
    Final(String),
    Error(String),
}

pub struct AliyunEngine {
    app_key: String,
    token_manager: Arc<TokenManager>,
    http_client: reqwest::Client,
    runtime: Handle,
    session: Option<AliyunSession>,
    current_partial: String,
}

struct AliyunSession {
    tx: mpsc::Sender<SessionCmd>,
    rx: mpsc::Receiver<AliyunEvent>,
}

enum SessionCmd {
    Audio(Vec<u8>),
    Stop,
}

impl AliyunEngine {
    pub fn new(
        app_key: impl Into<String>,
        token_manager: Arc<TokenManager>,
        runtime: Handle,
    ) -> Self {
        Self {
            app_key: app_key.into(),
            token_manager,
            http_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("valid Aliyun HTTP client configuration"),
            runtime,
            session: None,
            current_partial: String::new(),
        }
    }

    fn ensure_session(&mut self) -> anyhow::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let token = self.runtime.block_on(
            self.token_manager.get(&self.http_client)
        )?;
        let task_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let url = format!(
            "wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token={}",
            token
        );
        let app_key = self.app_key.clone();
        let start_msg = build_start_message(&app_key, &task_id);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCmd>(128);
        let (evt_tx, evt_rx) = mpsc::channel::<AliyunEvent>(64);

        let task_id_clone = task_id.clone();
        self.runtime.spawn(async move {
            let ws_result = connect_async(&url).await;
            let (mut ws, _) = match ws_result {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = evt_tx.send(AliyunEvent::Error(e.to_string())).await;
                    return;
                }
            };
            if let Err(e) = ws.send(Message::Text(start_msg.into())).await {
                let _ = evt_tx.send(AliyunEvent::Error(e.to_string())).await;
                return;
            }
            let mut ready = false;
            let mut pending_audio = std::collections::VecDeque::<Vec<u8>>::new();
            let mut stop_pending = false;
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SessionCmd::Audio(pcm)) => {
                                if ready {
                                    if ws.send(Message::Binary(pcm.into())).await.is_err() { break; }
                                } else {
                                    pending_audio.push_back(pcm);
                                }
                            }
                            Some(SessionCmd::Stop) => {
                                if ready {
                                    let stop = build_stop_message(&app_key, &task_id_clone);
                                    let _ = ws.send(Message::Text(stop.into())).await;
                                } else {
                                    stop_pending = true;
                                }
                            }
                            None => break,
                        }
                    }
                    msg = ws.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(evt) = parse_event(&text) {
                                    if matches!(evt, AliyunEvent::Ready) {
                                        ready = true;
                                        while let Some(pcm) = pending_audio.pop_front() {
                                            if ws.send(Message::Binary(pcm.into())).await.is_err() {
                                                return;
                                            }
                                        }
                                        if stop_pending {
                                            let stop = build_stop_message(&app_key, &task_id_clone);
                                            let _ = ws.send(Message::Text(stop.into())).await;
                                        }
                                    }
                                    let done = matches!(evt, AliyunEvent::Final(_) | AliyunEvent::Error(_));
                                    let _ = evt_tx.send(evt).await;
                                    if done { break; }
                                }
                            }
                            None | Some(Err(_)) => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        self.session = Some(AliyunSession { tx: cmd_tx, rx: evt_rx });
        Ok(())
    }

    fn drain_events(&mut self) {
        let mut failed = false;
        if let Some(ref mut sess) = self.session {
            while let Ok(evt) = sess.rx.try_recv() {
                match evt {
                    AliyunEvent::Ready => {}
                    AliyunEvent::Partial(t) => self.current_partial = t,
                    AliyunEvent::Final(t) => self.current_partial = t,
                    AliyunEvent::Error(e) => {
                        failed = true;
                        log::warn!("阿里云 ASR 事件错误，将在下一音频块重连: {e}");
                    }
                }
            }
        }
        if failed {
            self.session = None;
        }
    }
}

impl crate::SegmentEngine for AliyunEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        if let Err(e) = self.ensure_session() {
            log::error!("阿里云 ASR 建连失败: {e}");
            return None;
        }
        let pcm = f32_to_i16_pcm(samples);
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        if let Some(ref sess) = self.session {
            if let Err(e) = sess.tx.try_send(SessionCmd::Audio(bytes)) {
                log::warn!("阿里云 ASR 音频发送队列已满或关闭，本块音频未发送: {e}");
            }
        }
        self.drain_events();
        if self.current_partial.is_empty() { None } else { Some(self.current_partial.clone()) }
    }

    fn finish(&mut self) -> String {
        if let Some(ref sess) = self.session {
            let _ = sess.tx.try_send(SessionCmd::Stop);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut final_text = self.current_partial.clone();
        if let Some(ref mut sess) = self.session {
            while std::time::Instant::now() < deadline {
                match sess.rx.try_recv() {
                    Ok(AliyunEvent::Final(t)) => { final_text = t; break; }
                    Ok(AliyunEvent::Partial(t)) => { final_text = t; }
                    Ok(AliyunEvent::Error(e)) => {
                        // IDLE_TIMEOUT 在无语音段时属正常结束，不需要警告
                        if !e.contains("IDLE_TIMEOUT") {
                            log::warn!("finish 时收到错误: {e}");
                        }
                        break;
                    }
                    Ok(AliyunEvent::Ready) => {}
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        }
        // 段结束后总是重置 session，下段建立新连接，避免复用已关闭的后台 task
        self.session = None;
        self.current_partial.clear();
        final_text
    }

    fn reset(&mut self) {
        self.session = None;
        self.current_partial.clear();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::AliyunCloud
    }
}

pub(crate) fn build_start_message(app_key: &str, task_id: &str) -> String {
    serde_json::json!({
        "header": {
            "message_id": uuid::Uuid::new_v4().to_string().replace('-', ""),
            "task_id": task_id,
            "namespace": "SpeechTranscriber",
            "name": "StartTranscription",
            "appkey": app_key
        },
        "payload": {
            "format": "pcm",
            "sample_rate": 16000,
            "enable_intermediate_result": true,
            "enable_punctuation_prediction": true,
            "enable_inverse_text_normalization": true,
            "enable_semantic_sentence_detection": true
        }
    }).to_string()
}

pub(crate) fn build_stop_message(app_key: &str, task_id: &str) -> String {
    serde_json::json!({
        "header": {
            "message_id": uuid::Uuid::new_v4().to_string().replace('-', ""),
            "task_id": task_id,
            "namespace": "SpeechTranscriber",
            "name": "StopTranscription",
            "appkey": app_key
        },
        "payload": {}
    }).to_string()
}

fn parse_event(text: &str) -> Option<AliyunEvent> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = v["header"]["name"].as_str()?;
    match name {
        "TranscriptionStarted" => Some(AliyunEvent::Ready),
        "TranscriptionResultChanged" => {
            Some(AliyunEvent::Partial(v["payload"]["result"].as_str().unwrap_or("").to_string()))
        }
        "SentenceEnd" => {
            Some(AliyunEvent::Final(v["payload"]["result"].as_str().unwrap_or("").to_string()))
        }
        "TaskFailed" => {
            Some(AliyunEvent::Error(v["header"]["status_text"].as_str().unwrap_or("unknown").to_string()))
        }
        _ => None,
    }
}

pub(crate) fn f32_to_i16_pcm(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| {
        let clamped = s.clamp(-1.0, 1.0);
        (clamped * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_message_has_required_fields() {
        let msg = build_start_message("my-appkey", "task-uuid-001");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["header"]["name"], "StartTranscription");
        assert_eq!(v["header"]["appkey"], "my-appkey");
        assert_eq!(v["payload"]["format"], "pcm");
        assert_eq!(v["payload"]["sample_rate"], 16000);
        assert_eq!(v["payload"]["enable_intermediate_result"], true);
        assert_eq!(v["payload"]["enable_punctuation_prediction"], true);
        assert_eq!(v["payload"]["enable_semantic_sentence_detection"], true);
    }

    #[test]
    fn stop_message_has_required_fields() {
        let msg = build_stop_message("my-appkey", "task-uuid-001");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["header"]["name"], "StopTranscription");
    }

    #[test]
    fn f32_to_i16_pcm_clamps_correctly() {
        let samples = vec![0.0f32, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0];
        let out = f32_to_i16_pcm(&samples);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], i16::MAX);
        assert_eq!(out[2], i16::MIN);
        assert_eq!(out[5], i16::MAX);
        assert_eq!(out[6], i16::MIN);
    }
}
