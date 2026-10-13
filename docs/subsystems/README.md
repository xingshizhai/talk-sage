# Subsystem design references

These pages describe current responsibilities, runtime paths, state, and
contracts. Start with the subsystem relevant to your task, then follow its
source links to verify the implementation you change.

Adapters do not assemble the pipeline. The Tauri shell, the HTTP/WS server
and the CLI all enter through `TalkSageService`, then `SessionRuntime`, then
`LivePipeline`. A fix that only touches the carrier in the report is not
done until that shared path is checked.

| Subsystem | Reference |
| --- | --- |
| Audio capture, loopback, WAV | [architecture §8.1](../architecture-v2.md#81-音频域talksage-audio), [RECORDING.md](../RECORDING.md) — `crates/talksage-audio` |
| ASR, VAD, GPU, cloud fallback | [architecture §8.2](../architecture-v2.md#82-asr-域talksage-asr), [§20](../architecture-v2.md#20-asr-架构迁移2026-08-25gpu-加速--阿里云云端回退), [model management](../model-management.md), [real-time transcription](../real-time-transcription.md) — `crates/talksage-asr` |
| Pipeline, speakers, plugins | [architecture §8.3](../architecture-v2.md#83-管道与插件talksage-pipeline--talksage-plugins), [plugin development](../plugin-development.md) — `crates/talksage-pipeline`, `crates/talksage-plugins` |
| Session store | [architecture §8.4](../architecture-v2.md#84-会话域talksage-session) — `crates/talksage-session` |
| LLM | [architecture §8.5](../architecture-v2.md#85-llm-域talksage-llm) — `crates/talksage-llm` |
| Meeting notes | [architecture §8.6](../architecture-v2.md#86-纪要模板化talksage-notes) — `crates/talksage-notes` |
| Knowledge | [architecture §8.7](../architecture-v2.md#87-知识库talksage-knowledge) — `crates/talksage-knowledge` |
| Domain events | [architecture §9](../architecture-v2.md#9-事件协议与传输无关) — `crates/talksage-core` |
| Desktop UI | [architecture §10](../architecture-v2.md#10-前端设计web) — `web/` |
| Server and CLI | [architecture §4](../architecture-v2.md#4-总体架构), [CLI](../cli.md) — `crates/talksage-server`, `crates/talksage-cli` |
| Config and scenes | [architecture §13](../architecture-v2.md#13-配置管理talksage-config) — `crates/talksage-config` |

A subsystem can span several crates. Its reference describes the behavior
those parts provide together; it does not need to list every function.

When a change needs a tighter page than the architecture doc, add
`docs/subsystems/<name>.md`, link it from the table, and keep the fact in
that one page.

## Maintain a reference

Update the owning reference in the same PR when a change alters a documented
responsibility, interface, state transition, runtime path, compatibility
rule, or failure behavior. An internal refactor that preserves these
contracts does not require a prose change.

Keep each fact in one reference and link to it from agent instructions or
other pages. Link to source files and named symbols rather than copying
implementation details that change frequently. Explain field meanings and
constraints that signatures alone do not express.

For a new reference, include the responsibilities and boundaries, source of
truth, main runtime paths, state transitions, important constraints, and
existing verification commands. Include failure and compatibility behavior
where relevant. Document current behavior separately from proposed changes;
the proposal belongs in its issue or PR.

## Describe a PR's semantic changes

For each affected subsystem, explain the observable change with its trigger
and before/after behavior. Include changes to interfaces, state,
compatibility, and failure behavior when relevant. Link the affected
reference, implementation, and verification evidence. State when a refactor
preserves the documented behavior.

For example, a change to how a final segment is persisted should say which
carrier starts the session, which path commits the segment, what is written
before and after, and which test exercises that transition.

This is an author-provided explanation that a reviewer verifies against the
diff and surrounding code. It is not an automatically proven compatibility
verdict.

## Review a change

1. Identify the affected subsystem and the actual runtime path, including
   which carrier called `TalkSageService`.
2. Compare the claimed before/after behavior with the base and head
   implementation. Trace relevant callers and state transitions beyond the
   changed lines.
3. Check that changed contracts appear in the owning reference and that
   source links still resolve.
4. Check that the cited tests or runtime evidence exercise the claimed
   behavior. Report checks that were not run.

A documentation update alone does not prove the implementation. Existing
tests and runtime evidence remain necessary for behavior changes.
