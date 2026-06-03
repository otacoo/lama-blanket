use std::path::Path;

pub struct IconData {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn load_icon() -> IconData {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());

    let candidates = ["icon.png", "icon.webp", "icon.jpg", "icon.jpeg"];
    for name in &candidates {
        let path = exe_dir.join(name);
        if path.exists() {
            if let Ok(img) = image::open(&path) {
                let rgba = img.into_rgba8();
                let (w, h) = rgba.dimensions();
                return IconData {
                    rgba: rgba.into_raw(),
                    width: w,
                    height: h,
                };
            }
        }
    }

    make_default_icon()
}

fn make_default_icon() -> IconData {
    let size = 32u32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let border = x < 2 || x >= size - 2 || y < 2 || y >= size - 2;
            if border {
                pixels.push(255);
                pixels.push(255);
                pixels.push(255);
                pixels.push(255);
            } else {
                pixels.push(40);
                pixels.push(100);
                pixels.push(200);
                pixels.push(255);
            }
        }
    }
    IconData {
        rgba: pixels,
        width: size,
        height: size,
    }
}
