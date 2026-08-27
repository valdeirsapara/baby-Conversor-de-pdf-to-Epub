use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::pdf_model::ImageAsset;

/// Extracts every image XObject placed on a page, re-encoding raster samples we can
/// decode into PNG and passing already-JPEG (DCTDecode) data through untouched.
/// Images whose codec we cannot decode (JPXDecode/CCITTFax/JBIG2, exotic bit depths,
/// unsupported color spaces) are skipped with a log line rather than failing the run.
pub fn extract_page_images(
    doc: &Document,
    page_id: ObjectId,
    mut log: impl FnMut(String),
) -> Vec<(ObjectId, ImageAsset)> {
    let mut out = Vec::new();
    let images = match doc.get_page_images(page_id) {
        Ok(images) => images,
        Err(_) => return out,
    };
    for img in images {
        match build_asset(doc, &img) {
            Ok(asset) => out.push((img.id, asset)),
            Err(e) => log(format!(
                "imagem {}:{} ignorada ({e})",
                img.id.0, img.id.1
            )),
        }
    }
    out
}

fn build_asset(doc: &Document, img: &lopdf::xobject::PdfImage) -> Result<ImageAsset, String> {
    let filters = img.filters.clone().unwrap_or_default();
    let filename_base = format!("img_{}_{}", img.id.0, img.id.1);

    if filters.iter().any(|f| f == "DCTDecode") {
        if filters.len() == 1 {
            return Ok(ImageAsset {
                filename: format!("{filename_base}.jpg"),
                bytes: img.content.to_vec(),
                mime: "image/jpeg",
            });
        }
        return Err(format!("DCTDecode combinado com outros filtros ({filters:?})"));
    }
    if filters
        .iter()
        .any(|f| matches!(f.as_str(), "JPXDecode" | "CCITTFaxDecode" | "JBIG2Decode"))
    {
        return Err(format!("codec não suportado: {filters:?}"));
    }

    let stream = doc
        .get_object(img.id)
        .map_err(|e| e.to_string())?
        .as_stream()
        .map_err(|e| e.to_string())?;
    let raw = stream.get_plain_content().map_err(|e| e.to_string())?;

    let width = img.width.max(0) as u32;
    let height = img.height.max(0) as u32;
    let bpc = img.bits_per_component.unwrap_or(8);
    let cs_name = img.color_space.clone().unwrap_or_else(|| "DeviceGray".to_string());

    let dyn_img = decode_raster(&raw, width, height, bpc, &cs_name, img.origin_dict, doc)?;
    let mut buf = Vec::new();
    dyn_img
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(ImageAsset {
        filename: format!("{filename_base}.png"),
        bytes: buf,
        mime: "image/png",
    })
}

fn decode_raster(
    raw: &[u8],
    width: u32,
    height: u32,
    bpc: i64,
    cs_name: &str,
    origin_dict: &Dictionary,
    doc: &Document,
) -> Result<DynamicImage, String> {
    if width == 0 || height == 0 {
        return Err("dimensões zero".into());
    }
    match cs_name {
        "DeviceGray" | "CalGray" => match bpc {
            8 => {
                let buf = GrayImage::from_raw(width, height, raw.to_vec())
                    .ok_or("tamanho de buffer incompatível (gray8)")?;
                Ok(DynamicImage::ImageLuma8(buf))
            }
            1 => {
                let bits = unpack_bits(raw, width, height, 1);
                let samples: Vec<u8> = bits.into_iter().map(|v| if v == 0 { 0u8 } else { 255u8 }).collect();
                let buf = GrayImage::from_raw(width, height, samples)
                    .ok_or("tamanho de buffer incompatível (gray1)")?;
                Ok(DynamicImage::ImageLuma8(buf))
            }
            other => Err(format!("DeviceGray com {other} bits não suportado")),
        },
        "DeviceRGB" | "CalRGB" => match bpc {
            8 => {
                let buf = RgbImage::from_raw(width, height, raw.to_vec())
                    .ok_or("tamanho de buffer incompatível (rgb8)")?;
                Ok(DynamicImage::ImageRgb8(buf))
            }
            other => Err(format!("DeviceRGB com {other} bits não suportado")),
        },
        "DeviceCMYK" => match bpc {
            8 => {
                let rgb = cmyk_to_rgb(raw);
                let buf =
                    RgbImage::from_raw(width, height, rgb).ok_or("tamanho de buffer incompatível (cmyk)")?;
                Ok(DynamicImage::ImageRgb8(buf))
            }
            other => Err(format!("DeviceCMYK com {other} bits não suportado")),
        },
        "Indexed" => decode_indexed(raw, width, height, bpc, origin_dict, doc),
        other => Err(format!("espaço de cor não suportado: {other}")),
    }
}

fn cmyk_to_rgb(raw: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(raw.len());
    for px in raw.chunks_exact(4) {
        let (c, m, y, k) = (
            px[0] as f32 / 255.,
            px[1] as f32 / 255.,
            px[2] as f32 / 255.,
            px[3] as f32 / 255.,
        );
        rgb.push((255. * (1. - c) * (1. - k)) as u8);
        rgb.push((255. * (1. - m) * (1. - k)) as u8);
        rgb.push((255. * (1. - y) * (1. - k)) as u8);
    }
    rgb
}

fn decode_indexed(
    raw: &[u8],
    width: u32,
    height: u32,
    bpc: i64,
    origin_dict: &Dictionary,
    doc: &Document,
) -> Result<DynamicImage, String> {
    if bpc != 8 {
        return Err(format!("Indexed com {bpc} bits não suportado"));
    }
    let cs_obj = origin_dict.get(b"ColorSpace").map_err(|e| e.to_string())?;
    let (_, cs_obj) = doc.dereference(cs_obj).map_err(|e| e.to_string())?;
    let arr = cs_obj.as_array().map_err(|e| e.to_string())?;
    if arr.len() < 4 {
        return Err("Indexed ColorSpace malformado".into());
    }
    let (_, base_obj) = doc.dereference(&arr[1]).map_err(|e| e.to_string())?;
    let base_name = match base_obj {
        Object::Name(n) => String::from_utf8_lossy(n).to_string(),
        Object::Array(a) => a
            .first()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).to_string())
            .unwrap_or_default(),
        _ => return Err("base do Indexed não reconhecida".into()),
    };
    let base_components: usize = match base_name.as_str() {
        "DeviceRGB" | "CalRGB" => 3,
        "DeviceGray" | "CalGray" => 1,
        "DeviceCMYK" => 4,
        _ => return Err(format!("base do Indexed não suportada: {base_name}")),
    };
    let (_, lookup_obj) = doc.dereference(&arr[3]).map_err(|e| e.to_string())?;
    let palette: Vec<u8> = match lookup_obj {
        Object::String(bytes, _) => bytes.clone(),
        Object::Stream(s) => s.get_plain_content().map_err(|e| e.to_string())?,
        _ => return Err("tabela de paleta (lookup) não reconhecida".into()),
    };

    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for &idx in raw.iter().take(width as usize * height as usize) {
        let off = idx as usize * base_components;
        match base_components {
            3 => {
                rgb.push(*palette.get(off).unwrap_or(&0));
                rgb.push(*palette.get(off + 1).unwrap_or(&0));
                rgb.push(*palette.get(off + 2).unwrap_or(&0));
            }
            1 => {
                let g = *palette.get(off).unwrap_or(&0);
                rgb.push(g);
                rgb.push(g);
                rgb.push(g);
            }
            4 => {
                let px = [
                    *palette.get(off).unwrap_or(&0),
                    *palette.get(off + 1).unwrap_or(&0),
                    *palette.get(off + 2).unwrap_or(&0),
                    *palette.get(off + 3).unwrap_or(&0),
                ];
                let converted = cmyk_to_rgb(&px);
                rgb.extend_from_slice(&converted);
            }
            _ => unreachable!(),
        }
    }
    let buf = RgbImage::from_raw(width, height, rgb).ok_or("tamanho de buffer incompatível (indexed)")?;
    Ok(DynamicImage::ImageRgb8(buf))
}

/// Unpacks sub-byte-per-sample raster rows (PDF rows are byte-aligned, so each row
/// must be unpacked independently rather than treating the buffer as one long bitstream).
fn unpack_bits(raw: &[u8], width: u32, height: u32, bpc: u32) -> Vec<u8> {
    let row_bytes = ((width as u64 * bpc as u64 + 7) / 8) as usize;
    let mut out = Vec::with_capacity(width as usize * height as usize);
    for row in 0..height as usize {
        let start = row * row_bytes;
        if start >= raw.len() {
            out.extend(std::iter::repeat(0).take(width as usize));
            continue;
        }
        let end = (start + row_bytes).min(raw.len());
        let row_data = &raw[start..end];
        let mut bit_idx = 0usize;
        for _ in 0..width {
            let byte_idx = bit_idx / 8;
            let bit_off = 7 - (bit_idx % 8);
            let bit = row_data.get(byte_idx).map(|b| (b >> bit_off) & 1).unwrap_or(0);
            out.push(bit);
            bit_idx += bpc as usize;
        }
    }
    out
}
