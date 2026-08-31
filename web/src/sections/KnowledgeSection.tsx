// 知识管理：二级模块「专业术语」与「知识库」的独立管理页。
// 配置写入 talksage.toml（与「设置」同键，两边保存互相同步）；知识库文档列表只读 + 钉选管理。

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import type { AppConfig } from "../lib/api";
import { getApi } from "../lib/transport";
import { knowledgeBaseSettings, knowledgeSourceReady, truncateNoteText, type KnowledgeDoc } from "../lib/knowledge";

const api = getApi();

type KbTab = "terms" | "kb";

const TAB_META: { key: KbTab; label: string; desc: string }[] = [
  { key: "terms", label: "专业术语", desc: "热词与误识别替换" },
  { key: "kb", label: "知识库", desc: "本地仓库 / 会中材料包" },
];

const inputStyle: CSSProperties = {
  fontSize: 12,
  padding: "4px 8px",
  borderRadius: 4,
  border: "1px solid var(--border)",
  background: "var(--surface-2)",
  color: "var(--text)",
  boxSizing: "border-box",
};
const numStyle: CSSProperties = { ...inputStyle, width: 90, marginLeft: 8 };
const labelBlock: CSSProperties = { display: "block", marginBottom: 4 };
const hint: CSSProperties = { marginTop: 4, color: "var(--muted)", fontSize: 11, lineHeight: 1.6 };
const groupTitle: CSSProperties = { margin: "0 0 6px", fontSize: 13 };

export default function KnowledgeSection({
  config,
  onSave,
  pinnedNotePaths,
  onTogglePin,
}: {
  config: AppConfig | null;
  onSave: (updates: Record<string, unknown>) => Promise<void>;
  /** 会中材料包：已钉住的笔记路径（与转写页右栏共用一份状态）。 */
  pinnedNotePaths: string[];
  onTogglePin: (path: string) => void;
}) {
  const [tab, setTab] = useState<KbTab>("terms");

  // ── 专业术语 ─────────────────────────────────────────────
  const [termsEnabled, setTermsEnabled] = useState(config?.asr?.terminology?.enabled ?? false);
  const [hotwordScore, setHotwordScore] = useState(config?.asr?.terminology?.hotword_score ?? 1.5);
  const [termsText, setTermsText] = useState((config?.asr?.terminology?.terms ?? []).join("\n"));
  const [correctionsText, setCorrectionsText] = useState(
    Object.entries(config?.asr?.terminology?.corrections ?? {})
      .map(([wrong, right]) => `${wrong} => ${right}`)
      .join("\n"),
  );

  // ── 知识库 ───────────────────────────────────────────────
  const kb = knowledgeBaseSettings(config);
  const [kbEnabled, setKbEnabled] = useState(kb.enabled);
  const [kbFolder, setKbFolder] = useState(kb.folder);
  const [docs, setDocs] = useState<KnowledgeDoc[]>([]);
  const [expandedDoc, setExpandedDoc] = useState<Record<string, boolean>>({});

  const [saving, setSaving] = useState(false);
  // 保存结果消息按 tab 隔离，避免切页后显示另一模块的提示
  const [msg, setMsg] = useState<{ tab: KbTab; text: string; error: boolean } | null>(null);

  /** 拉取已索引文档；源未启用/无路径时清空。 */
  const refreshDocs = useCallback(() => {
    if (!knowledgeSourceReady(config)) {
      setDocs([]);
      return;
    }
    api
      .listKnowledgeDocuments()
      .then(setDocs)
      .catch((e) => {
        console.error("读取知识库文档失败:", e);
        setDocs([]);
      });
  }, [config]);

  useEffect(() => {
    if (tab === "kb") refreshDocs();
  }, [tab, refreshDocs]);

  /** 保存专业术语配置（asr.terminology，与「设置 → 术语纠错」同键）。 */
  async function handleSaveTerms() {
    setSaving(true);
    setMsg(null);
    try {
      const corrections = Object.fromEntries(
        correctionsText
          .split("\n")
          .map((line) => {
            const [wrong, ...right] = line.split("=>");
            return [wrong?.trim(), right.join("=>").trim()];
          })
          .filter(([wrong, right]) => wrong && right),
      );
      await onSave({
        asr: {
          terminology: {
            enabled: termsEnabled,
            hotword_score: hotwordScore,
            terms: termsText.split("\n").map((v) => v.trim()).filter(Boolean),
            corrections,
          },
        },
      });
      setMsg({ tab: "terms", text: "术语配置已保存，下次监听生效", error: false });
    } catch (e) {
      setMsg({ tab: "terms", text: `保存失败: ${e}`, error: true });
    } finally {
      setSaving(false);
    }
  }

  /** 保存知识库配置（knowledge_base + knowledge_obsidian 插件，与「设置 → 插件」同键）。 */
  async function handleSaveKb() {
    setSaving(true);
    setMsg(null);
    try {
      await onSave({
        knowledge_base: { enabled: kbEnabled, folder: kbFolder.trim() },
        plugins: { knowledge_obsidian: { enabled: kbEnabled, folder: kbFolder.trim() } },
      });
      refreshDocs();
      setMsg({ tab: "kb", text: "知识库配置已保存，索引已刷新", error: false });
    } catch (e) {
      setMsg({ tab: "kb", text: `保存失败: ${e}`, error: true });
    } finally {
      setSaving(false);
    }
  }

  async function handlePickKbFolder() {
    try {
      const path = await api.pickFolder();
      if (path) {
        setKbFolder(path);
        setKbEnabled(true);
      }
    } catch (e) {
      setMsg({ tab: "kb", text: String(e), error: true });
    }
  }

  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10, fontSize: 12, display: "flex", flexDirection: "column", height: "100%", minHeight: 0, boxSizing: "border-box" }}>
      {/* 二级模块切换 */}
      <div style={{ display: "flex", gap: 6, marginBottom: 10, flexWrap: "wrap" }}>
        {TAB_META.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            title={t.desc}
            style={{
              fontSize: 12,
              padding: "4px 12px",
              borderRadius: 7,
              cursor: "pointer",
              border: tab === t.key ? "1px solid var(--me)" : "1px solid var(--border)",
              background: tab === t.key ? "var(--me-soft)" : "var(--surface-2)",
              color: tab === t.key ? "var(--text)" : "var(--text-2)",
              fontWeight: 600,
            }}
          >
            {t.label}
            <span style={{ marginLeft: 6, fontSize: 10, color: "var(--muted)", fontWeight: 400 }}>{t.desc}</span>
          </button>
        ))}
      </div>

      <div style={{ flex: 1, minHeight: 0, overflowY: "auto" }}>
        {/* ── 专业术语 ── */}
        {tab === "terms" && (
          <div>
            <h3 style={groupTitle}>专业术语增强</h3>
            <label style={labelBlock}>
              <input type="checkbox" checked={termsEnabled} onChange={(e) => setTermsEnabled(e.target.checked)} /> 启用会议术语上下文
            </label>
            <textarea
              value={termsText}
              onChange={(e) => setTermsText(e.target.value)}
              disabled={!termsEnabled}
              placeholder={"每行一个术语，例如：\nTalkSage\nParaformer\n向量数据库"}
              rows={5}
              style={{ ...inputStyle, width: "100%", resize: "vertical", opacity: termsEnabled ? 1 : 0.5 }}
            />
            <label style={{ ...labelBlock, opacity: termsEnabled ? 1 : 0.5 }}>
              热词强度：
              <input
                type="number"
                min={0}
                max={10}
                step={0.1}
                disabled={!termsEnabled}
                value={hotwordScore}
                onChange={(e) => setHotwordScore(Math.min(10, Math.max(0, Number(e.target.value) || 0)))}
                style={numStyle}
              />
            </label>
            <textarea
              value={correctionsText}
              onChange={(e) => setCorrectionsText(e.target.value)}
              disabled={!termsEnabled}
              placeholder={"常见误识别 => 标准术语，例如：\n怕热佛母 => Paraformer"}
              rows={4}
              style={{ ...inputStyle, width: "100%", resize: "vertical", opacity: termsEnabled ? 1 : 0.5 }}
            />
            <div style={hint}>Zipformer 与 Qwen3-ASR 会使用模型热词；Paraformer 使用纠错表。纠错同步作用于 partial/final，不增加模型推理延迟。修改后下次监听生效。</div>
            <div style={{ marginTop: 10, display: "flex", gap: 8, alignItems: "center" }}>
              <button onClick={() => void handleSaveTerms()} disabled={saving} style={{ fontSize: 12, padding: "4px 14px", cursor: saving ? "default" : "pointer" }}>
                {saving ? "保存中…" : "保存术语配置"}
              </button>
              {msg && msg.tab === "terms" && (
                <span style={{ fontSize: 11, color: msg.error ? "var(--danger)" : "var(--live)" }}>{msg.text}</span>
              )}
            </div>
          </div>
        )}

        {/* ── 知识库 ── */}
        {tab === "kb" && (
          <div>
            <h3 style={groupTitle}>知识源：Obsidian / 本地仓库</h3>
            <label style={labelBlock}>
              <input type="checkbox" checked={kbEnabled} onChange={(e) => setKbEnabled(e.target.checked)} /> 启用本地仓库
            </label>
            <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
              <input
                value={kbFolder}
                onChange={(e) => setKbFolder(e.target.value)}
                placeholder="Obsidian 仓库路径，例如 D:\Obsidian"
                style={{ ...inputStyle, flex: 1 }}
              />
              <button type="button" onClick={() => void handlePickKbFolder()} style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer", flexShrink: 0 }}>
                浏览…
              </button>
            </div>
            <div style={hint}>保存后立即刷新索引，供会中材料包、纪要和 AI 助手引用。只支持一个 vault 根路径；.obsidian / .trash / .git 不会进索引。</div>
            <div style={{ marginTop: 10, display: "flex", gap: 8, alignItems: "center" }}>
              <button onClick={() => void handleSaveKb()} disabled={saving} style={{ fontSize: 12, padding: "4px 14px", cursor: saving ? "default" : "pointer" }}>
                {saving ? "保存中…" : "保存知识库配置"}
              </button>
              {msg && msg.tab === "kb" && (
                <span style={{ fontSize: 11, color: msg.error ? "var(--danger)" : "var(--live)" }}>{msg.text}</span>
              )}
            </div>

            {/* 已索引文档 + 材料包钉选 */}
            <div style={{ marginTop: 14, borderTop: "1px dashed var(--border)", paddingTop: 10 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
                <b style={{ fontSize: 13 }}>已索引文档</b>
                <span style={{ fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{docs.length}</span>
                <button
                  onClick={refreshDocs}
                  style={{ marginLeft: "auto", fontSize: 11, padding: "2px 10px", borderRadius: 6, cursor: "pointer", border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--muted)" }}
                >
                  刷新
                </button>
              </div>
              {!knowledgeSourceReady(config) && <div style={{ ...hint, marginTop: 0 }}>未启用或未填写仓库路径：保存上面的配置后即可看到文档列表。</div>}
              {knowledgeSourceReady(config) && docs.length === 0 && <div style={{ ...hint, marginTop: 0 }}>仓库里还没有可用的笔记（.md / .txt）。</div>}
              <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 4 }}>
                {docs.map((doc) => {
                  const pinned = pinnedNotePaths.includes(doc.path);
                  const expanded = !!expandedDoc[doc.path];
                  return (
                    <div key={doc.path} style={{ border: "1px solid var(--border)", borderRadius: 8, background: "var(--surface-2)", padding: "6px 9px" }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <input
                          type="checkbox"
                          checked={pinned}
                          title={pinned ? "取消钉住（不再进入会中材料包）" : "钉住为会中材料包"}
                          onChange={() => onTogglePin(doc.path)}
                          style={{ cursor: "pointer", flexShrink: 0 }}
                        />
                        <span
                          onClick={() => setExpandedDoc((prev) => ({ ...prev, [doc.path]: !prev[doc.path] }))}
                          title={expanded ? "收起" : "展开全文"}
                          style={{ flex: 1, minWidth: 0, cursor: "pointer" }}
                        >
                          <b style={{ fontSize: 12 }}>{doc.title || doc.path}</b>
                          <span style={{ display: "block", fontSize: 10, color: "var(--muted)", fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                            {doc.path}
                          </span>
                        </span>
                        <span style={{ fontSize: 10, color: "var(--muted)", flexShrink: 0 }}>{expanded ? "▾" : "▸"}</span>
                      </div>
                      {expanded && (
                        <div style={{ marginTop: 6, paddingTop: 6, borderTop: "1px dashed var(--border)", fontSize: 11, lineHeight: 1.6, color: "var(--text-2)", wordBreak: "break-word" }}>
                          {truncateNoteText(doc.text, 1200)}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
              <div style={hint}>勾选的笔记进入「会中材料包」：转写页右栏与会议开始时可对照引用（与右栏的钉选同步）。</div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
