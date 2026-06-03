use crate::gguf;
use crate::hwdetect;

#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub model_path: String,
    pub mmproj_path: String,
    pub host: String,
    pub port: u16,
    pub ctx_size: u64,
    pub gpu_layers: u64,
    pub threads: usize,
    pub threads_batch: usize,
    pub batch_size: u64,
    pub ubatch_size: u64,
    pub flash_attn: String,
    pub cache_type_k: String,
    pub cache_type_v: String,
    pub mlock: bool,
    pub split_mode: String,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            mmproj_path: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            ctx_size: 4096,
            gpu_layers: 0,
            threads: 4,
            threads_batch: 4,
            batch_size: 2048,
            ubatch_size: 512,
            flash_attn: "auto".into(),
            cache_type_k: "q8_0".into(),
            cache_type_v: "q8_0".into(),
            mlock: false,
            split_mode: String::new(),
        }
    }
}

pub struct ResourceEstimate {
    pub model_size_mb: u64,
    pub kv_cache_mb: u64,
    pub overhead_mb: u64,
    pub total_vram_mb: u64,
    pub total_ram_mb: u64,
    pub gpu_layers: u64,
    pub total_layers: u64,
    pub fits_vram: bool,
    pub fits_ram: bool,
}

const VRAM_OVERHEAD_MB: u64 = 1024;

pub fn compute(
    info: &gguf::GgufInfo,
    hw: &hwdetect::HardwareInfo,
    overrides: Option<&LaunchConfig>,
) -> LaunchConfig {
    let total_layers = info.block_count;
    let model_size_mb = info.file_size / (1024 * 1024);

    let ctx_size = overrides
        .and_then(|o| if o.ctx_size > 0 { Some(o.ctx_size) } else { None })
        .unwrap_or_else(|| {
            if info.context_length > 0 {
                info.context_length
            } else {
                4096
            }
        })
        .min(128_000);

    let per_layer_mb = if total_layers > 0 {
        model_size_mb as f64 / total_layers as f64
    } else {
        model_size_mb as f64
    };

    let cache_dtype = overrides
        .map(|o| o.cache_type_k.as_str())
        .unwrap_or("q8_0");
    let kv_cache_mb = estimate_kv_cache(ctx_size, info, cache_dtype);

    let gpu = hw.gpus.first();
    let vram_mb = gpu.map(|g| g.vram_mb).unwrap_or(0);
    let has_gpu = vram_mb > 0;

    let gpu_layers = overrides
        .and_then(|o| if o.gpu_layers > 0 { Some(o.gpu_layers) } else { None })
        .unwrap_or_else(|| {
            if has_gpu {
                let usable_vram = vram_mb.saturating_sub(VRAM_OVERHEAD_MB) as f64;
                let vram_for_layers = usable_vram - kv_cache_mb as f64;
                if vram_for_layers <= 0.0 {
                    0
                } else {
                    let layers = (vram_for_layers / per_layer_mb) as u64;
                    layers.min(total_layers)
                }
            } else {
                0
            }
        });

    let threads = overrides
        .and_then(|o| if o.threads > 0 { Some(o.threads) } else { None })
        .unwrap_or(hw.physical_cores.max(1));

    let threads_batch = overrides
        .and_then(|o| if o.threads_batch > 0 { Some(o.threads_batch) } else { None })
        .unwrap_or(hw.logical_cores.max(1));

    let batch_size = overrides
        .map(|o| o.batch_size)
        .unwrap_or(2048);

    let ubatch_size = overrides
        .map(|o| o.ubatch_size)
        .unwrap_or(512);

    let flash_attn = overrides
        .map(|o| o.flash_attn.clone())
        .unwrap_or_else(|| {
            if hw.has_nvidia && has_gpu && info.architecture == "llama" {
                "auto".into()
            } else {
                "off".into()
            }
        });

    let cache_type_k = overrides
        .map(|o| o.cache_type_k.clone())
        .unwrap_or_else(|| "q8_0".into());
    let cache_type_v = overrides
        .map(|o| o.cache_type_v.clone())
        .unwrap_or_else(|| "q8_0".into());

    let mlock = overrides.map(|o| o.mlock).unwrap_or_else(|| {
        let ram_avail_mb = hw.available_ram_mb;
        ram_avail_mb > model_size_mb * 3 / 2
    });

    let host = overrides
        .map(|o| o.host.clone())
        .unwrap_or_else(|| "127.0.0.1".into());
    let port = overrides
        .and_then(|o| if o.port > 0 { Some(o.port) } else { None })
        .unwrap_or(8080);
    let model_path = overrides
        .map(|o| o.model_path.clone())
        .unwrap_or_default();
    let mmproj_path = overrides
        .map(|o| o.mmproj_path.clone())
        .unwrap_or_default();

    let split_mode = overrides
        .map(|o| o.split_mode.clone())
        .unwrap_or_default();

    LaunchConfig {
        model_path,
        mmproj_path,
        host,
        port,
        ctx_size,
        gpu_layers,
        threads,
        threads_batch,
        batch_size,
        ubatch_size,
        flash_attn,
        cache_type_k,
        cache_type_v,
        mlock,
        split_mode,
    }
}

pub fn estimate_resources(
    info: &gguf::GgufInfo,
    config: &LaunchConfig,
    hw: &hwdetect::HardwareInfo,
) -> ResourceEstimate {
    let model_size_mb = info.file_size / (1024 * 1024);
    let total_layers = info.block_count;

    let kv_cache_mb = estimate_kv_cache(config.ctx_size, info, &config.cache_type_k);

    let vram_mb = hw.gpus.first().map(|g| g.vram_mb).unwrap_or(0);
    let gpu_model_mb = if total_layers > 0 {
        (model_size_mb as f64 * config.gpu_layers as f64 / total_layers as f64) as u64
    } else {
        0
    };
    let cpu_model_mb = model_size_mb.saturating_sub(gpu_model_mb);

    let total_vram_mb = if config.gpu_layers > 0 {
        gpu_model_mb + kv_cache_mb + VRAM_OVERHEAD_MB
    } else {
        0
    };

    let total_ram_mb = cpu_model_mb + if config.gpu_layers == 0 { kv_cache_mb } else { 0 };

    ResourceEstimate {
        model_size_mb,
        kv_cache_mb,
        overhead_mb: VRAM_OVERHEAD_MB,
        total_vram_mb,
        total_ram_mb,
        gpu_layers: config.gpu_layers,
        total_layers,
        fits_vram: total_vram_mb <= vram_mb || config.gpu_layers == 0,
        fits_ram: total_ram_mb <= hw.available_ram_mb,
    }
}

fn estimate_kv_cache(ctx_size: u64, info: &gguf::GgufInfo, cache_type: &str) -> u64 {
    let n_layers = info.block_count.max(1);
    let n_kv_heads = if info.head_count_kv > 0 {
        info.head_count_kv
    } else {
        info.head_count
    }
    .max(1);
    let head_dim = if info.head_count > 0 && info.embedding_length > 0 {
        info.embedding_length / info.head_count
    } else {
        128
    };

    let bytes_per_element: u64 = match cache_type {
        "f16" => 2,
        "q8_0" => 1,
        "q4_0" => 1,
        _ => 1,
    };

    2 * n_layers * n_kv_heads * head_dim * ctx_size * bytes_per_element / (1024 * 1024)
}

impl LaunchConfig {
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        args.push("-m".into());
        args.push(self.model_path.clone());

        if !self.mmproj_path.is_empty() {
            args.push("--mmproj".into());
            args.push(self.mmproj_path.clone());
        }

        args.push("--host".into());
        args.push(self.host.clone());

        args.push("--port".into());
        args.push(self.port.to_string());

        if self.ctx_size > 0 {
            args.push("-c".into());
            args.push(self.ctx_size.to_string());
        }

        if self.gpu_layers > 0 {
            args.push("-ngl".into());
            args.push(self.gpu_layers.to_string());
        }

        if self.threads > 0 {
            args.push("-t".into());
            args.push(self.threads.to_string());
        }

        if self.threads_batch > 0 {
            args.push("-tb".into());
            args.push(self.threads_batch.to_string());
        }

        if self.batch_size > 0 {
            args.push("-b".into());
            args.push(self.batch_size.to_string());
        }

        if self.ubatch_size > 0 {
            args.push("-ub".into());
            args.push(self.ubatch_size.to_string());
        }

        if !self.flash_attn.is_empty() {
            args.push("-fa".into());
            args.push(self.flash_attn.clone());
        }

        if !self.cache_type_k.is_empty() {
            args.push("-ctk".into());
            args.push(self.cache_type_k.clone());
        }

        if !self.cache_type_v.is_empty() {
            args.push("-ctv".into());
            args.push(self.cache_type_v.clone());
        }

        if self.mlock {
            args.push("--mlock".into());
        }

        if !self.split_mode.is_empty() {
            args.push("-sm".into());
            args.push(self.split_mode.clone());
        }

        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_info() -> gguf::GgufInfo {
        gguf::GgufInfo {
            architecture: "llama".into(),
            block_count: 40,
            context_length: 8192,
            embedding_length: 4096,
            head_count: 32,
            head_count_kv: 8,
            file_type: 15,
            file_size: 6_500_000_000,
            model_name: "Test Model".into(),
        }
    }

    fn make_hw(vram_mb: u64, ram_avail_mb: u64) -> hwdetect::HardwareInfo {
        hwdetect::HardwareInfo {
            cpu_name: "Test CPU".into(),
            physical_cores: 16,
            logical_cores: 32,
            total_ram_mb: ram_avail_mb + 4096,
            available_ram_mb: ram_avail_mb,
            gpus: vec![hwdetect::GpuInfo {
                name: "NVIDIA RTX 4070 Ti".into(),
                vram_mb,
            }],
            has_nvidia: true,
        }
    }

    #[test]
    fn test_full_gpu_offload() {
        let info = make_info();
        let hw = make_hw(12 * 1024, 24 * 1024);
        let config = compute(&info, &hw, None);

        assert_eq!(config.gpu_layers, 40);
        assert_eq!(config.ctx_size, 8192);
        assert_eq!(config.threads, 16);
        assert_eq!(config.flash_attn, "auto");
        assert_eq!(config.cache_type_k, "q8_0");
    }

    #[test]
    fn test_cpu_only() {
        let info = make_info();
        let hw = make_hw(0, 24 * 1024);
        let config = compute(&info, &hw, None);

        assert_eq!(config.gpu_layers, 0);
        assert_eq!(config.threads, 16);
        assert_eq!(config.flash_attn, "off");
    }

    #[test]
    fn test_overrides() {
        let info = make_info();
        let hw = make_hw(12 * 1024, 24 * 1024);
        let overrides = LaunchConfig {
            ctx_size: 4096,
            threads: 8,
            gpu_layers: 20,
            flash_attn: "off".into(),
            ..Default::default()
        };
        let config = compute(&info, &hw, Some(&overrides));

        assert_eq!(config.ctx_size, 4096);
        assert_eq!(config.threads, 8);
        assert_eq!(config.gpu_layers, 20);
        assert_eq!(config.flash_attn, "off");
    }

    #[test]
    fn test_resource_estimate() {
        let info = make_info();
        let config = compute(&info, &make_hw(12 * 1024, 24 * 1024), None);
        let est = estimate_resources(&info, &config, &make_hw(12 * 1024, 24 * 1024));

        assert_eq!(est.gpu_layers, 40);
        assert!(est.total_vram_mb > 0);
        assert!(est.total_vram_mb < 12 * 1024);
        assert!(est.fits_vram);
    }

    #[test]
    fn test_to_args() {
        let config = LaunchConfig {
            model_path: "test.gguf".into(),
            mmproj_path: "test-mmproj.gguf".into(),
            host: "0.0.0.0".into(),
            port: 9090,
            ctx_size: 4096,
            gpu_layers: 40,
            threads: 8,
            threads_batch: 16,
            batch_size: 2048,
            ubatch_size: 512,
            flash_attn: "auto".into(),
            cache_type_k: "q8_0".into(),
            cache_type_v: "q8_0".into(),
            mlock: true,
            split_mode: String::new(),
        };

        let args = config.to_args();
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"test.gguf".to_string()));
        assert!(args.contains(&"--port".to_string()));
        assert!(args.contains(&"9090".to_string()));
        assert!(args.contains(&"--mlock".to_string()));
        assert!(args.contains(&"-ctk".to_string()));
        assert!(args.contains(&"q8_0".to_string()));
    }

    #[test]
    fn test_kv_cache_estimation() {
        let info = make_info();
        let kv = estimate_kv_cache(8192, &info, "q8_0");
        assert!(kv > 100);
        assert!(kv < 2000);

        let kv_f16 = estimate_kv_cache(8192, &info, "f16");
        assert!(kv_f16 > kv);
    }
}
