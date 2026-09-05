// Miniaturas reales de imagen (GRAMMAR.md §3.258) -- ver el comentario en
// `compiler/Cargo.toml` sobre por qué `image` (image-rs) es la séptima
// excepción real a "cero dependencias nuevas". Alcance v1, deliberadamente
// chico: decodifica JPEG/PNG/GIF/BMP/WebP (los cinco formatos que un
// backend real recibe de un cliente/navegador), redimensiona preservando
// relación de aspecto dentro de una caja `maxWidth x maxHeight` (nunca
// distorsiona, nunca recorta), y reescribe en el MISMO formato detectado --
// salvo GIF/WebP, que colapsan a PNG (ver `output_format_for`, la razón está
// documentada ahí). Filtro de resize FIJO (Lanczos3, buena calidad a costo
// razonable) -- sin parámetro de configuración, mismo criterio que el
// A4/márgenes fijos de `pdf.rs`.
//
// Límites honestos, deliberados:
// - Sin orientación EXIF: una foto de un celular en portrait con la bandera
//   EXIF de rotación puede salir "acostada" -- `image` no la aplica sola, y
//   corregirla necesitaría leer el bloque EXIF además de la imagen (fuera de
//   alcance de v1, sin evidencia real de que haga falta todavía).
// - Un GIF/WebP animado pierde su animación: solo el primer frame sobrevive,
//   reescrito como PNG estático -- generar un GIF/WebP animado de vuelta es
//   un problema de codificación de video/animación, no de miniaturas.
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageFormat};
use std::io::Cursor;

/// Decide en qué formato reescribir la miniatura. JPEG/PNG/BMP se preservan
/// tal cual -- el caso real (una foto vs. un gráfico con transparencia) ya
/// eligió el formato correcto de origen, y cambiarlo sería una sorpresa no
/// pedida. GIF/WebP colapsan a PNG: como de todos modos solo el primer frame
/// sobrevive (ver el comentario de arriba), reescribirlos como GIF/WebP de
/// nuevo daría un archivo "animado" de un solo frame -- más confuso que
/// útil, y PNG sin pérdida es un destino más honesto para lo que en la
/// práctica ya es una imagen estática.
fn output_format_for(detected: ImageFormat) -> ImageFormat {
    match detected {
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Bmp => detected,
        _ => ImageFormat::Png,
    }
}

/// `qualified_name` es "image.thumbnail" o "image.dimensions" -- mismo
/// criterio que `excel::parse` (prefija el nombre calificado en el mensaje
/// de error, no solo en el call site de `runtime/mod.rs`) para que un
/// mensaje de error sea reconocible sin tener que mirar de qué builtin vino.
fn decode(bytes: &[u8], qualified_name: &str) -> Result<(DynamicImage, ImageFormat), String> {
    let format = image::guess_format(bytes).map_err(|e| {
        format!("{qualified_name}: no se pudo identificar el formato de la imagen (¿son bytes de imagen reales?): {e}")
    })?;
    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("{qualified_name}: no se pudo decodificar la imagen ({format:?}): {e}"))?;
    Ok((img, format))
}

/// `image.thumbnail(base64, maxWidth, maxHeight)` -- redimensiona para que
/// entre en la caja `maxWidth x maxHeight` preservando relación de aspecto
/// (`DynamicImage::resize`, nunca recorta ni distorsiona), y reescribe en el
/// formato que decide `output_format_for`. `maxWidth`/`maxHeight` aceptan
/// agrandar una imagen más chica que la caja tanto como achicar una más
/// grande -- sin un caso real que pida "solo achicar, nunca agrandar"
/// todavía.
pub fn thumbnail(bytes: &[u8], max_width: i64, max_height: i64) -> Result<Vec<u8>, String> {
    if max_width <= 0 || max_height <= 0 {
        return Err(format!(
            "image.thumbnail: maxWidth/maxHeight tienen que ser mayores a 0 (recibido {max_width}x{max_height})"
        ));
    }
    let (img, detected) = decode(bytes, "image.thumbnail")?;
    let resized = img.resize(max_width as u32, max_height as u32, FilterType::Lanczos3);
    let out_format = output_format_for(detected);
    let mut buf = Cursor::new(Vec::new());
    // JPEG no tiene canal alfa -- escribir un `DynamicImage` con transparencia
    // (ej. un PNG de origen reescrito a JPEG por venir de un formato que
    // colapsa, aunque hoy PNG nunca colapsa a JPEG) directo falla en el
    // encoder de `image`; convertir a RGB8 primero descarta el alfa a
    // propósito, la única opción sensata para un formato sin ese canal.
    let result = if out_format == ImageFormat::Jpeg {
        resized.to_rgb8().write_to(&mut buf, out_format)
    } else {
        resized.write_to(&mut buf, out_format)
    };
    result.map_err(|e| format!("image.thumbnail: no se pudo codificar el resultado como {out_format:?}: {e}"))?;
    Ok(buf.into_inner())
}

/// `image.dimensions(base64)` -- ancho/alto reales de la imagen decodificada,
/// sin redimensionar nada. Mismo decode que `thumbnail`, sin la segunda
/// pasada de resize/encode.
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let (img, _) = decode(bytes, "image.dimensions")?;
    Ok(img.dimensions())
}
