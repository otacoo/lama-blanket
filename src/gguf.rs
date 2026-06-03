use std::fs;
use std::io::{self, Read};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Not a valid GGUF file: {0}")]
    InvalidFormat(String),
    #[error("Unsupported GGUF version: {0}")]
    UnsupportedVersion(u32),
}

#[derive(Debug, Clone)]
pub struct GgufInfo {
    pub architecture: String,
    pub block_count: u64,
    pub context_length: u64,
    pub embedding_length: u64,
    pub head_count: u64,
    pub head_count_kv: u64,
    pub file_type: u32,
    pub file_size: u64,
    pub model_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GgufType {
    U8 = 0,
    I8 = 1,
    U16 = 2,
    I16 = 3,
    U32 = 4,
    I32 = 5,
    F32 = 6,
    Bool = 7,
    String = 8,
    Array = 9,
    U64 = 10,
    I64 = 11,
    F64 = 12,
}

impl GgufType {
    fn from_u32(v: u32) -> Result<Self, GgufError> {
        match v {
            0 => Ok(Self::U8),
            1 => Ok(Self::I8),
            2 => Ok(Self::U16),
            3 => Ok(Self::I16),
            4 => Ok(Self::U32),
            5 => Ok(Self::I32),
            6 => Ok(Self::F32),
            7 => Ok(Self::Bool),
            8 => Ok(Self::String),
            9 => Ok(Self::Array),
            10 => Ok(Self::U64),
            11 => Ok(Self::I64),
            12 => Ok(Self::F64),
            _ => Err(GgufError::InvalidFormat(format!("Unknown value type: {v}"))),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    String(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Array {
        elem_type: GgufType,
        values: Vec<GgufValue>,
    },
}

pub fn parse_gguf(path: &Path) -> Result<GgufInfo, GgufError> {
    let file_size = fs::metadata(path)?.len();
    let data = fs::read(path)?;
    let mut cursor = io::Cursor::new(&data[..]);

    let magic = read_exact(&mut cursor, 4)?;
    if &magic != b"GGUF" {
        return Err(GgufError::InvalidFormat("Missing GGUF magic bytes".into()));
    }

    let version = read_u32(&mut cursor)?;
    if version < 2 || version > 3 {
        return Err(GgufError::UnsupportedVersion(version));
    }

    let tensor_count = read_u64(&mut cursor)?;
    let kv_count = read_u64(&mut cursor)?;

    let mut architecture = String::new();
    let mut block_count: u64 = 0;
    let mut context_length: u64 = 0;
    let mut embedding_length: u64 = 0;
    let mut head_count: u64 = 0;
    let mut head_count_kv: u64 = 0;
    let mut file_type: u32 = 0;
    let mut model_name = String::new();

    for _ in 0..kv_count {
        let key = read_string(&mut cursor)?;
        let val_type = GgufType::from_u32(read_u32(&mut cursor)?)?;
        let value = read_value(&mut cursor, &val_type)?;

        match key.as_str() {
            "general.architecture" => {
                if let GgufValue::String(ref s) = value {
                    architecture = s.clone();
                }
            }
            "general.name" => {
                if let GgufValue::String(ref s) = value {
                    model_name = s.clone();
                }
            }
            "general.file_type" => {
                if let GgufValue::U32(v) = value {
                    file_type = v;
                }
            }
            _ => {}
        }

        if !architecture.is_empty() {
            let arch_ctx_key = format!("{architecture}.context_length");
            let arch_block_key = format!("{architecture}.block_count");
            let arch_embd_key = format!("{architecture}.embedding_length");
            let arch_head_key = format!("{architecture}.attention.head_count");
            let arch_head_kv_key = format!("{architecture}.attention.head_count_kv");

            if key == arch_ctx_key {
                match &value {
                    GgufValue::U64(v) => context_length = *v,
                    GgufValue::U32(v) => context_length = *v as u64,
                    _ => {}
                }
            }
            if key == arch_block_key {
                match &value {
                    GgufValue::U64(v) => block_count = *v,
                    GgufValue::U32(v) => block_count = *v as u64,
                    _ => {}
                }
            }
            if key == arch_embd_key {
                match &value {
                    GgufValue::U64(v) => embedding_length = *v,
                    GgufValue::U32(v) => embedding_length = *v as u64,
                    _ => {}
                }
            }
            if key == arch_head_key {
                match &value {
                    GgufValue::U64(v) => head_count = *v,
                    GgufValue::U32(v) => head_count = *v as u64,
                    _ => {}
                }
            }
            if key == arch_head_kv_key {
                match &value {
                    GgufValue::U64(v) => head_count_kv = *v,
                    GgufValue::U32(v) => head_count_kv = *v as u64,
                    _ => {}
                }
            }
        }
    }

    if architecture.is_empty() {
        return Err(GgufError::InvalidFormat(
            "No general.architecture found in metadata".into(),
        ));
    }

    if block_count == 0 {
        return Err(GgufError::InvalidFormat(
            "No block_count found for architecture".into(),
        ));
    }

    if model_name.is_empty() {
        model_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
    }

    let _ = tensor_count;

    Ok(GgufInfo {
        architecture,
        block_count,
        context_length,
        embedding_length,
        head_count,
        head_count_kv,
        file_type,
        file_size,
        model_name,
    })
}

fn read_exact(cursor: &mut io::Cursor<&[u8]>, n: usize) -> Result<Vec<u8>, io::Error> {
    let mut buf = vec![0u8; n];
    cursor.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_u32(cursor: &mut io::Cursor<&[u8]>) -> Result<u32, io::Error> {
    let bytes = read_exact(cursor, 4)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64(cursor: &mut io::Cursor<&[u8]>) -> Result<u64, io::Error> {
    let bytes = read_exact(cursor, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i64(cursor: &mut io::Cursor<&[u8]>) -> Result<i64, io::Error> {
    let bytes = read_exact(cursor, 8)?;
    Ok(i64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_f64(cursor: &mut io::Cursor<&[u8]>) -> Result<f64, io::Error> {
    let bytes = read_exact(cursor, 8)?;
    Ok(f64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_f32(cursor: &mut io::Cursor<&[u8]>) -> Result<f32, io::Error> {
    let bytes = read_exact(cursor, 4)?;
    Ok(f32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_bool(cursor: &mut io::Cursor<&[u8]>) -> Result<bool, io::Error> {
    let bytes = read_exact(cursor, 1)?;
    Ok(bytes[0] != 0)
}

fn read_string(cursor: &mut io::Cursor<&[u8]>) -> Result<String, io::Error> {
    let len = read_u64(cursor)? as usize;
    let bytes = read_exact(cursor, len)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_value(
    cursor: &mut io::Cursor<&[u8]>,
    val_type: &GgufType,
) -> Result<GgufValue, io::Error> {
    match val_type {
        GgufType::U8 => {
            let b = read_exact(cursor, 1)?;
            Ok(GgufValue::U8(b[0]))
        }
        GgufType::I8 => {
            let b = read_exact(cursor, 1)?;
            Ok(GgufValue::I8(b[0] as i8))
        }
        GgufType::U16 => {
            let b = read_exact(cursor, 2)?;
            Ok(GgufValue::U16(u16::from_le_bytes(b.try_into().unwrap())))
        }
        GgufType::I16 => {
            let b = read_exact(cursor, 2)?;
            Ok(GgufValue::I16(i16::from_le_bytes(b.try_into().unwrap())))
        }
        GgufType::U32 => Ok(GgufValue::U32(read_u32(cursor)?)),
        GgufType::I32 => Ok(GgufValue::I32(read_u32(cursor)? as i32)),
        GgufType::F32 => Ok(GgufValue::F32(read_f32(cursor)?)),
        GgufType::Bool => Ok(GgufValue::Bool(read_bool(cursor)?)),
        GgufType::String => Ok(GgufValue::String(read_string(cursor)?)),
        GgufType::U64 => Ok(GgufValue::U64(read_u64(cursor)?)),
        GgufType::I64 => Ok(GgufValue::I64(read_i64(cursor)?)),
        GgufType::F64 => Ok(GgufValue::F64(read_f64(cursor)?)),
        GgufType::Array => {
            let elem_type_u32 = read_u32(cursor)?;
            let elem_type = GgufType::from_u32(elem_type_u32)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            let count = read_u64(cursor)?;
            let mut values = Vec::with_capacity(count as usize);
            for _ in 0..count {
                values.push(read_value(cursor, &elem_type)?);
            }
            Ok(GgufValue::Array {
                elem_type,
                values,
            })
        }
    }
}

pub fn file_type_name(ft: u32) -> &'static str {
    match ft {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        6 => "Q5_0",
        7 => "Q8_0",
        8 => "Q8_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q4_K_L",
        17 => "Q5_K_S",
        18 => "Q5_K_M",
        19 => "Q5_K_L",
        20 => "Q6_K",
        21 => "IQ2_XXS",
        22 => "IQ2_XS",
        23 => "IQ3_XXS",
        24 => "IQ3_S",
        25 => "IQ3_M",
        26 => "IQ4_XS",
        27 => "IQ4_NL",
        _ => "unknown",
    }
}

pub fn quant_bits_per_parameter(ft: u32) -> f64 {
    match ft {
        0 => 32.0,
        1 => 16.0,
        2 => 4.0,
        3 => 4.5,
        6 => 5.0,
        7 => 8.0,
        8 => 8.5,
        10 => 2.0,
        11 => 2.9,
        12 => 3.0,
        13 => 3.1,
        14 => 4.0,
        15 => 4.3,
        16 => 4.6,
        17 => 5.0,
        18 => 5.3,
        19 => 5.6,
        20 => 6.0,
        _ => file_type_default_bpp(ft),
    }
}

fn file_type_default_bpp(ft: u32) -> f64 {
    if ft < 2 {
        return 16.0;
    }
    if ft < 6 {
        return 4.5;
    }
    if ft < 10 {
        return 8.0;
    }
    if ft < 14 {
        return 3.0;
    }
    if ft < 17 {
        return 4.5;
    }
    if ft < 20 {
        return 5.5;
    }
    if ft < 28 {
        return 3.5;
    }
    4.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_names() {
        assert_eq!(file_type_name(0), "F32");
        assert_eq!(file_type_name(1), "F16");
        assert_eq!(file_type_name(15), "Q4_K_M");
        assert_eq!(file_type_name(99), "unknown");
    }

    #[test]
    fn test_quant_bits_per_parameter() {
        assert_eq!(quant_bits_per_parameter(0), 32.0);
        assert_eq!(quant_bits_per_parameter(1), 16.0);
        assert_eq!(quant_bits_per_parameter(7), 8.0);
        assert_eq!(quant_bits_per_parameter(15), 4.3);
    }
}
