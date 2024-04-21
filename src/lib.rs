extern crate rusttype;

use worker::*;

use ab_glyph::{point, Font, FontRef, Glyph, Point, PxScale, ScaleFont};
use image::{ImageBuffer, Rgba};
use log;

const IMAGE_WIDTH: u32 = 1200;
const IMAGE_HEIGHT: u32 = 630;

const INTERNAL_SERVER_ERROR: &str = "Internal Server Error";

fn query(req: &Request, key: &str) -> Option<String> {
    req.url()
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string())
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    if req.method() != Method::Get {
        return Response::error("Method Not Allowed".to_string(), 405);
    }

    let url = match req.url() {
        Ok(url) => url,
        _ => {
            log::error!("failed to get url");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    let cache = Cache::default();
    let cache_key = CacheKey::Url(url.to_string());

    let cached = match cache.get(cache_key, false).await {
        Ok(cached) => cached,
        Err(e) => {
            log::error!("failed to get cache: {e}");
            None
        }
    };
    match cached {
        Some(cached) => {
            return Ok(cached);
        }
        None => {
            log::info!("cache not found (url = {url})");
        }
    }

    let text = match query(&req, "text") {
        Some(text) => {
            if text.len() > 150 {
                return Response::error("text parameter is too long".to_string(), 400);
            }
            text
        }
        None => {
            return Response::error("text parameter is required".to_string(), 400);
        }
    };
    let author = match query(&req, "author") {
        Some(author) => author,
        None => {
            return Response::error("author parameter is required".to_string(), 400);
        }
    };
    let title = match query(&req, "title") {
        Some(title) => title,
        None => {
            return Response::error("title parameter is required".to_string(), 400);
        }
    };

    let bucket = match env.bucket("BUCKET") {
        Ok(bucket) => bucket,
        Err(e) => {
            log::error!("failed to get bucket: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    let raw_font = match bucket.get("MPLUS1p-Medium.ttf").execute().await {
        Ok(raw_font) => match raw_font {
            Some(raw_font) => raw_font,
            None => {
                log::error!("font is not found");
                return Response::error(INTERNAL_SERVER_ERROR.to_string(), 404);
            }
        },
        Err(e) => {
            log::error!("failed to get font: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    let raw_font = raw_font.body().unwrap().bytes().await.unwrap();

    let font = match FontRef::try_from_slice(&raw_font) {
        Ok(font) => font,
        Err(e) => {
            log::error!("failed to load font: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };

    let mut imgbuf = ImageBuffer::from_pixel(IMAGE_WIDTH, IMAGE_HEIGHT, Rgba([255, 255, 255, 255]));
    imgbuf = render_text(
        font.clone(),
        PxScale::from(70.0),
        imgbuf,
        &text,
        (0, 0, 0),
        point(80.0, 230.0),
    );
    imgbuf = render_text(
        font.clone(),
        PxScale::from(60.0),
        imgbuf,
        &title,
        (0, 0, 0),
        point(80.0, 80.0),
    );
    imgbuf = render_text(
        font.clone(),
        PxScale::from(60.0),
        imgbuf,
        &author,
        (0, 0, 0),
        point(1000.0, 500.0),
    );

    let mut buffer = std::io::Cursor::new(vec![]);
    match imgbuf.write_to(&mut buffer, image::ImageFormat::Png) {
        Ok(_) => {}
        Err(e) => {
            log::error!("failed to write image: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    }

    let resp = match Response::from_bytes(buffer.into_inner()) {
        Ok(resp) => resp,
        Err(e) => {
            log::error!("failed to create response: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    let mut headers = Headers::new();
    match headers.set("content-type", "image/png") {
        Ok(_) => {}
        Err(e) => {
            log::error!("failed to set content-type header: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    match headers.set("Cache-Control", "public, max-age=604800") {
        // 1 week
        Ok(_) => {}
        Err(e) => {
            log::error!("failed to set Cache-Control header: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    let mut resp = resp.with_headers(headers);
    let cloned_resp = match resp.cloned() {
        Ok(cloned_resp) => cloned_resp,
        Err(e) => {
            log::error!("failed to clone response: {e}");
            return Response::error(INTERNAL_SERVER_ERROR.to_string(), 500);
        }
    };
    match cache.put(url.to_string(), cloned_resp).await {
        Ok(_) => {}
        Err(e) => {
            // cache に保存できなくてもレスポンスは返す
            log::error!("failed to put cache: {e}");
        }
    };

    Ok(resp)
}

fn render_text<F: Font>(
    font: F,
    font_scale: PxScale,
    imgbuf: ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    text_color: (u8, u8, u8),
    text_position: Point,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let scaled_font = font.as_scaled(font_scale);

    let mut glyphs = Vec::new();
    layout_paragraph(
        scaled_font,
        text_position,
        IMAGE_WIDTH as f32 - 180.0,
        text,
        &mut glyphs,
    );

    render_glyphs(font, glyphs, imgbuf, text_color)
}

fn render_glyphs<F: Font>(
    font: F,
    glyphs: Vec<Glyph>,
    mut imgbuf: ImageBuffer<Rgba<u8>, Vec<u8>>,
    text_color: (u8, u8, u8),
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    for glyph in glyphs {
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|x, y, v| {
                let px = imgbuf.get_pixel_mut(x + bounds.min.x as u32, y + bounds.min.y as u32);
                *px = Rgba([
                    (px.0[0] as f32 * (1.0 - v) + text_color.0 as f32 * v) as u8,
                    (px.0[1] as f32 * (1.0 - v) + text_color.1 as f32 * v) as u8,
                    (px.0[2] as f32 * (1.0 - v) + text_color.2 as f32 * v) as u8,
                    255,
                ]);
            });
        }
    }
    imgbuf
}

fn layout_paragraph<F, SF>(
    font: SF,
    position: Point,
    max_width: f32,
    text: &str,
    target: &mut Vec<Glyph>,
) where
    F: Font,
    SF: ScaleFont<F>,
{
    let v_advance = font.height() + font.line_gap();
    let mut caret = point(position.x, position.y + font.ascent());
    let mut last_glyph: Option<Glyph> = None;
    for c in text.chars() {
        if c.is_control() {
            if c == '\n' {
                caret = point(position.x, caret.y + v_advance);
                last_glyph = None;
            }
            continue;
        }
        let mut glyph = font.scaled_glyph(c);
        if let Some(previous) = last_glyph.take() {
            caret.x += font.kern(previous.id, glyph.id);
        }
        glyph.position = caret;

        last_glyph = Some(glyph.clone());
        caret.x += font.h_advance(glyph.id);

        if !c.is_whitespace() && caret.x > position.x + max_width {
            caret = point(position.x, caret.y + v_advance);
            last_glyph = None;
        }

        target.push(glyph);
    }
}
