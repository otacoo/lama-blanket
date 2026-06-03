use sysinfo::System;

#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: u64,
}

#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub cpu_name: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub gpus: Vec<GpuInfo>,
    pub has_nvidia: bool,
}

pub fn detect() -> HardwareInfo {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "Unknown CPU".into());

    let physical_cores = num_cpus::get_physical();
    let logical_cores = num_cpus::get();

    let total_ram_bytes = sys.total_memory();
    let available_ram_bytes = sys.available_memory();
    let total_ram_mb = total_ram_bytes / (1024 * 1024);
    let available_ram_mb = available_ram_bytes / (1024 * 1024);

    let mut gpus = Vec::new();
    let mut has_nvidia = false;
    let mut seen_names = std::collections::HashSet::new();

    match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            let device_count = nvml.device_count().unwrap_or(0);
            for i in 0..device_count {
                if let Ok(device) = nvml.device_by_index(i) {
                    if let Ok(name) = device.name() {
                        has_nvidia = true;
                        let vram_bytes = device.memory_info().map(|m| m.total).unwrap_or(0);
                        let vram_mb = vram_bytes / (1024 * 1024);
                        seen_names.insert(name.clone());
                        gpus.push(GpuInfo { name, vram_mb });
                    }
                }
            }
        }
        Err(_) => {}
    }

    detect_vulkan_gpus(&mut gpus, &mut seen_names);

    if gpus.is_empty() {
        gpus.push(GpuInfo {
            name: "No GPU detected".into(),
            vram_mb: 0,
        });
    }

    HardwareInfo {
        cpu_name,
        physical_cores,
        logical_cores,
        total_ram_mb,
        available_ram_mb,
        gpus,
        has_nvidia,
    }
}

fn detect_vulkan_gpus(gpus: &mut Vec<GpuInfo>, seen_names: &mut std::collections::HashSet<String>) {
    let entry = match unsafe { ash::Entry::load() } {
        Ok(e) => e,
        Err(_) => return,
    };

    let app_name = std::ffi::CString::new("lama-blanket").unwrap();
    let engine_name = std::ffi::CString::new("no-engine").unwrap();
    let app_info = ash::vk::ApplicationInfo {
        p_application_name: app_name.as_ptr(),
        application_version: 0,
        p_engine_name: engine_name.as_ptr(),
        engine_version: 0,
        api_version: ash::vk::make_api_version(0, 1, 0, 0),
        ..Default::default()
    };

    let create_info = ash::vk::InstanceCreateInfo {
        p_application_info: &app_info,
        ..Default::default()
    };

    let instance = match unsafe { entry.create_instance(&create_info, None) } {
        Ok(i) => i,
        Err(_) => return,
    };

    let devices = match unsafe { instance.enumerate_physical_devices() } {
        Ok(d) => d,
        Err(_) => {
            unsafe { instance.destroy_instance(None) };
            return;
        }
    };

    for &device in &devices {
        let props = unsafe { instance.get_physical_device_properties(device) };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(device) };

        let device_type = props.device_type;
        if device_type != ash::vk::PhysicalDeviceType::DISCRETE_GPU
            && device_type != ash::vk::PhysicalDeviceType::INTEGRATED_GPU
        {
            continue;
        }

        let name = String::from_utf8_lossy(
            &props.device_name
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as u8)
                .collect::<Vec<u8>>(),
        )
        .to_string();

        if seen_names.contains(&name) {
            continue;
        }
        seen_names.insert(name.clone());

        let mut max_heap = 0u64;
        for i in 0..mem_props.memory_heap_count as usize {
            let heap = mem_props.memory_heaps[i];
            if heap.flags.contains(ash::vk::MemoryHeapFlags::DEVICE_LOCAL) {
                max_heap = max_heap.max(heap.size);
            }
        }
        let vram_mb = max_heap / (1024 * 1024);

        gpus.push(GpuInfo { name, vram_mb });
    }

    unsafe { instance.destroy_instance(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect() {
        let hw = detect();
        assert!(hw.physical_cores > 0);
        assert!(hw.logical_cores > 0);
        assert!(hw.total_ram_mb > 0);
        assert!(!hw.cpu_name.is_empty());
        assert!(!hw.gpus.is_empty());
    }
}
