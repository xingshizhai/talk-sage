//! 插件注册表：Plugin trait、三类钩子、配置载体。

use serde_json::Value;

/// 插件配置载体。用 serde_json::Value 与 ConfigManager 已有的
/// apply_scene_params(p, u: &Value) 模式保持一致，不引入新的 schema 机制。
#[derive(Debug, Clone)]
pub struct PluginConfig(Value);

impl Default for PluginConfig {
    fn default() -> Self {
        Self(Value::Object(Default::default()))
    }
}

impl PluginConfig {
    pub fn from_value(v: Value) -> Self {
        Self(v)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// 用户值覆盖：只覆盖 user 里出现的键，其余保留默认。
    pub fn merge(&mut self, user: &Value) {
        let (Value::Object(base), Value::Object(over)) = (&mut self.0, user) else {
            return;
        };
        for (k, v) in over {
            base.insert(k.clone(), v.clone());
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.0.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn get_u64(&self, key: &str, default: u64) -> u64 {
        self.0.get(key).and_then(Value::as_u64).unwrap_or(default)
    }

    pub fn get_str(&self, key: &str, default: &str) -> String {
        self.0.get(key).and_then(Value::as_str).unwrap_or(default).to_string()
    }

    /// 约定键：所有插件都有 enabled，缺省为 true。
    pub fn enabled(&self) -> bool {
        self.get_bool("enabled", true)
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_values_override_defaults_and_unknown_keys_are_kept() {
        let mut cfg = PluginConfig::from_value(json!({"enabled": true, "cooldown_seconds": 30.0}));
        cfg.merge(&json!({"cooldown_seconds": 5.0}));
        assert_eq!(cfg.get_f64("cooldown_seconds", 0.0), 5.0);
        assert!(cfg.enabled(), "未覆盖的 enabled 应保留默认值");
    }

    #[test]
    fn missing_keys_fall_back_to_the_supplied_default() {
        let cfg = PluginConfig::from_value(json!({}));
        assert_eq!(cfg.get_u64("min_ms", 300), 300);
        assert_eq!(cfg.get_f64("ratio", 0.5), 0.5);
        assert!(cfg.get_bool("whatever", true));
    }

    #[test]
    fn enabled_defaults_to_true_and_can_be_switched_off() {
        assert!(PluginConfig::from_value(json!({})).enabled());
        assert!(!PluginConfig::from_value(json!({"enabled": false})).enabled());
    }
}
