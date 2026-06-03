use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ImportedPresetFile {
    One(NamedPreset),
    Many(Vec<NamedPreset>),
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub path: String,
    pub filename: String,
    pub size_mb: u64,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConfig {
    pub models_folder: Option<String>,
    pub last_model: Option<String>,
    pub ctx_size: Option<u64>,
    pub gpu_layers: Option<u64>,
    pub threads: Option<usize>,
    pub threads_batch: Option<usize>,
    pub batch_size: Option<u64>,
    pub cache_type_k: Option<String>,
    pub flash_attn: Option<String>,
    pub port: Option<u16>,
    pub mlock: Option<bool>,
    pub auto_launch: Option<bool>,
    pub dark_mode: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedPreset {
    pub name: String,
    pub model_path: Option<String>,
    pub ctx_size: Option<u64>,
    pub gpu_layers: Option<u64>,
    pub threads: Option<usize>,
    pub threads_batch: Option<usize>,
    pub batch_size: Option<u64>,
    pub cache_type_k: Option<String>,
    pub flash_attn: Option<String>,
    pub port: Option<u16>,
    pub mlock: Option<bool>,
}

impl Default for SavedConfig {
    fn default() -> Self {
        Self {
            models_folder: None,
            last_model: None,
            ctx_size: None,
            gpu_layers: None,
            threads: None,
            threads_batch: None,
            batch_size: None,
            cache_type_k: None,
            flash_attn: None,
            port: None,
            mlock: None,
            auto_launch: None,
            dark_mode: None,
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("."))
        .with_file_name("lama-blanket.json")
}

fn presets_path() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("."))
        .with_file_name("lama-blanket-presets.json")
}

pub fn load_config() -> SavedConfig {
    let path = config_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        SavedConfig::default()
    }
}

pub fn save_config(config: &SavedConfig) {
    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = fs::write(path, json);
    }
}

pub fn load_presets() -> Vec<NamedPreset> {
    let path = presets_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    }
}

pub fn save_presets(presets: &[NamedPreset]) {
    let path = presets_path();
    if let Ok(json) = serde_json::to_string_pretty(presets) {
        let _ = fs::write(path, json);
    }
}

pub fn import_presets(path: &Path) -> Result<Vec<NamedPreset>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    parse_presets(&contents)
}

pub fn parse_presets(contents: &str) -> Result<Vec<NamedPreset>, String> {
    let imported: ImportedPresetFile = serde_json::from_str(contents)
        .map_err(|e| format!("Invalid preset file: {e}"))?;

    let presets = match imported {
        ImportedPresetFile::One(preset) => vec![preset],
        ImportedPresetFile::Many(presets) => presets,
    }
    .into_iter()
    .filter_map(normalize_preset)
    .collect::<Vec<_>>();

    if presets.is_empty() {
        Err("No valid presets found in the selected file.".into())
    } else {
        Ok(presets)
    }
}

pub fn merge_presets(existing: &mut Vec<NamedPreset>, imported: Vec<NamedPreset>) -> (usize, usize) {
    let mut added = 0;
    let mut replaced = 0;

    for preset in imported {
        if let Some(existing_preset) = existing.iter_mut().find(|p| p.name == preset.name) {
            *existing_preset = preset;
            replaced += 1;
        } else {
            existing.push(preset);
            added += 1;
        }
    }

    sort_presets(existing);
    (added, replaced)
}

pub fn scan_models_folder(folder: &str) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    let root = Path::new(folder);
    if !root.is_dir() {
        return entries;
    }
    walk_dir(root, root, &mut entries);
    entries.sort_by(|a, b| a.filename.to_lowercase().cmp(&b.filename.to_lowercase()));
    entries
}

fn walk_dir(root: &Path, dir: &Path, entries: &mut Vec<ModelEntry>) {
    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_dir(root, &path, entries);
            } else if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("gguf") {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    if filename.to_lowercase().contains("mmproj") {
                        continue;
                    }
                    let size_mb = fs::metadata(&path)
                        .map(|m| m.len() / (1024 * 1024))
                        .unwrap_or(0);

                    let author = path
                        .strip_prefix(root)
                        .ok()
                        .and_then(|rel| rel.components().next())
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .filter(|a| {
                            a != "."
                                && !a.is_empty()
                                && Path::new(root).join(a) != path
                        })
                        .unwrap_or_default();

                    entries.push(ModelEntry {
                        path: path.to_string_lossy().to_string(),
                        filename,
                        size_mb,
                        author,
                    });
                }
            }
        }
    }
}

fn normalize_preset(mut preset: NamedPreset) -> Option<NamedPreset> {
    preset.name = preset.name.trim().to_string();
    if preset.name.is_empty() {
        None
    } else {
        Some(preset)
    }
}

fn sort_presets(presets: &mut [NamedPreset]) {
    presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
}

#[cfg(windows)]
pub fn set_auto_launch(enable: bool) {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"Software\Microsoft\Windows\CurrentVersion\Run";
    if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_SET_VALUE) {
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let _ = key.set_value("lama-blanket", &exe.to_string_lossy().to_string());
            }
        } else {
            let _ = key.delete_value("lama-blanket");
        }
    }
}

#[cfg(not(windows))]
pub fn set_auto_launch(_enable: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_preset_file() {
        let parsed = parse_presets(
            r#"{
                "name": "Imported",
                "ctx_size": 4096,
                "gpu_layers": 20
            }"#,
        )
        .unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Imported");
        assert_eq!(parsed[0].ctx_size, Some(4096));
    }

    #[test]
    fn parses_multiple_presets_file() {
        let parsed = parse_presets(
            r#"[
                { "name": "B", "ctx_size": 2048 },
                { "name": "A", "ctx_size": 4096 }
            ]"#,
        )
        .unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "B");
        assert_eq!(parsed[1].name, "A");
    }

    #[test]
    fn merge_presets_adds_and_replaces_by_name() {
        let mut existing = vec![NamedPreset {
            name: "Base".into(),
            model_path: None,
            ctx_size: Some(2048),
            gpu_layers: Some(8),
            threads: None,
            threads_batch: None,
            batch_size: None,
            cache_type_k: None,
            flash_attn: None,
            port: None,
            mlock: None,
        }];

        let imported = vec![
            NamedPreset {
                name: "Base".into(),
                model_path: None,
                ctx_size: Some(4096),
                gpu_layers: Some(16),
                threads: None,
                threads_batch: None,
                batch_size: None,
                cache_type_k: None,
                flash_attn: None,
                port: None,
                mlock: None,
            },
            NamedPreset {
                name: "Vision".into(),
                model_path: None,
                ctx_size: Some(8192),
                gpu_layers: Some(24),
                threads: None,
                threads_batch: None,
                batch_size: None,
                cache_type_k: None,
                flash_attn: None,
                port: None,
                mlock: None,
            },
        ];

        let (added, replaced) = merge_presets(&mut existing, imported);

        assert_eq!(added, 1);
        assert_eq!(replaced, 1);
        assert_eq!(existing.len(), 2);
        assert_eq!(existing[0].name, "Base");
        assert_eq!(existing[0].ctx_size, Some(4096));
        assert_eq!(existing[1].name, "Vision");
    }
}

