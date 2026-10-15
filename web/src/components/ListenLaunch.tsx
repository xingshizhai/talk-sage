import { useState } from "react";
import type { SceneMode, SceneParams } from "../lib/api";
import {
  FEATURE_LABELS,
  enabledPlugins,
  launchSummary,
  offeredPlugins,
  tasksEnabled,
  toggleFeature,
  type LaunchChoice,
} from "../lib/launch";

export default function ListenLaunch({
  sceneMode,
  custom,
  choice,
  onChange,
}: {
  sceneMode: SceneMode;
  custom?: Pick<SceneParams, "plugin_allowlist" | "translation_mode">;
  choice: LaunchChoice;
  onChange: (choice: LaunchChoice) => void;
}) {
  const [open, setOpen] = useState(false);
  const offered = offeredPlugins(sceneMode, custom);
  const enabled = enabledPlugins(choice, offered);
  const keyPointsOn = enabled.includes("key_point_llm");

  return (
    <div style={{ margin: "0 10px 8px", display: "flex", flexDirection: "column", gap: 6 }}>
      <div style={{ fontSize: 11, color: "var(--text)", lineHeight: 1.4 }}>
        {launchSummary(choice, offered)}
      </div>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        style={{
          border: "none",
          background: "none",
          padding: 0,
          cursor: "pointer",
          textAlign: "left",
          fontSize: 11,
          color: "var(--muted)",
        }}
      >
        {open ? "收起本次功能" : "调整本次功能"}
      </button>
      {open && (
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          {offered.map((feature) => (
            <FeatureCheck
              key={feature}
              label={FEATURE_LABELS[feature]}
              checked={enabled.includes(feature)}
              onChange={() => onChange(toggleFeature(choice, feature, offered))}
            />
          ))}
          {offered.includes("key_point_llm") && (
            <FeatureCheck
              label={FEATURE_LABELS.tasks}
              checked={tasksEnabled(choice, offered)}
              disabled={!keyPointsOn}
              onChange={() => onChange(toggleFeature(choice, "tasks", offered))}
            />
          )}
        </div>
      )}
    </div>
  );
}

function FeatureCheck({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: () => void;
}) {
  return (
    <label style={{ fontSize: 11, color: "var(--text)", display: "flex", gap: 6, alignItems: "center", opacity: disabled ? 0.45 : 1 }}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={onChange} />
      {label}
    </label>
  );
}
