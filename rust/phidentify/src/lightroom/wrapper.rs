use anyhow::{Result, ensure};
use md5::{Digest, Md5};

pub fn wrap(
    raw_digest: &[u8; 16],
    model: &str,
    crop_size: [(u32, u32); 2],
    crop_origin: [(u32, u32); 2],
    user_crop: Option<[(u32, u32); 4]>,
    opcode_suffix: &[u8],
) -> Result<[u8; 16]> {
    ensure!(
        !model.is_empty() && !model.as_bytes().contains(&0),
        "invalid camera model"
    );
    let mut hash = Md5::new();
    hash.update(b"DNG\0Image\0");
    hash.update(raw_digest);
    hash.update(model.as_bytes());
    hash.update([0]);
    for (numerator, denominator) in crop_size.into_iter().chain(crop_origin) {
        ensure!(denominator != 0, "zero crop denominator");
        hash.update(numerator.to_le_bytes());
        hash.update(denominator.to_le_bytes());
    }
    if let Some(crop) = user_crop {
        for (numerator, denominator) in crop {
            ensure!(denominator != 0, "zero user crop denominator");
            hash.update(numerator.to_le_bytes());
            hash.update(denominator.to_le_bytes());
        }
    }
    hash.update(opcode_suffix);
    Ok(hash.finalize().into())
}
