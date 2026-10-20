//! X11 capture through checked requests. A removed drawable never falls back to root.
//! GStreamer's ximagesrc uses an Xlib error path which can exit the process when
//! the selected window disappears; keeping capture in Rust avoids that path.
use crate::capture::{CaptureKind, CaptureSourceInfo};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        randr::ConnectionExt as _,
        xfixes::ConnectionExt as _,
        xproto::{self, ConnectionExt as _},
    },
    rust_connection::RustConnection,
};

struct Capture {
    conn: RustConnection,
    root: u32,
    selected: CaptureSourceInfo,
    cursor: bool,
    next: Instant,
    dimensions: (u16, u16),
}
impl Capture {
    fn new(selected: CaptureSourceInfo) -> Result<Self, String> {
        let (conn, index) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[index].root;
        if conn.setup().image_byte_order != xproto::ImageOrder::LSB_FIRST {
            return Err("Ordem de pixels X11 não suportada".into());
        }
        if selected.kind == CaptureKind::Window {
            conn.change_window_attributes(
                selected.id as u32,
                &xproto::ChangeWindowAttributesAux::new()
                    .event_mask(xproto::EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        }
        let cursor = conn
            .xfixes_query_version(5, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        let mut capture = Self {
            conn,
            root,
            selected,
            cursor,
            next: Instant::now(),
            dimensions: (0, 0),
        };
        let (_, _, _, width, height) = capture.region()?;
        capture.dimensions = (width, height);
        Ok(capture)
    }
    fn region(&self) -> Result<(u32, i16, i16, u16, u16), String> {
        if self.selected.kind == CaptureKind::Window {
            let id = u32::try_from(self.selected.id).map_err(|_| "Janela inválida")?;
            let geometry = self
                .conn
                .get_geometry(id)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|_| "A janela foi encerrada")?;
            let format = self
                .conn
                .setup()
                .pixmap_formats
                .iter()
                .find(|f| f.depth == geometry.depth)
                .ok_or("Formato de imagem X11 ausente")?;
            if format.bits_per_pixel != 32 {
                return Err("A captura X11 requer pixels de 32 bits".into());
            }
            Ok((id, 0, 0, geometry.width, geometry.height))
        } else {
            let monitors = self
                .conn
                .randr_get_monitors(self.root, true)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
            let monitor = monitors
                .monitors
                .iter()
                .find(|m| m.name as u64 == self.selected.id)
                .ok_or("O monitor foi removido")?;
            Ok((
                self.root,
                monitor.x,
                monitor.y,
                monitor.width,
                monitor.height,
            ))
        }
    }
    fn frame(&mut self, src: &gst_app::AppSrc) -> Result<gst::Buffer, String> {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now)
        }
        self.next = Instant::now() + Duration::from_millis(1000 / 15);
        while let Some(event) = self.conn.poll_for_event().map_err(|e| e.to_string())? {
            if matches!(event,Event::DestroyNotify(event) if event.window as u64==self.selected.id)
            {
                return Err("A janela foi encerrada".into());
            }
        }
        let (drawable, x, y, width, height) = self.region()?;
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(4))
            .filter(|v| *v > 0 && *v <= 256 * 1024 * 1024)
            .ok_or("Dimensões de captura inválidas")?;
        let image = self
            .conn
            .get_image(
                xproto::ImageFormat::Z_PIXMAP,
                drawable,
                x,
                y,
                width,
                height,
                u32::MAX,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|_| "A fonte X11 deixou de estar disponível")?;
        if image.data.len() != bytes {
            return Err("Formato de imagem X11 incompatível".into());
        }
        if self.dimensions != (width, height) {
            src.set_caps(Some(&caps(width, height)));
            self.dimensions = (width, height);
        }
        let mut pixels = image.data;
        if self.cursor {
            self.cursor(&mut pixels, drawable, x, y, width, height);
        }
        let mut buffer = gst::Buffer::from_mut_slice(pixels);
        buffer
            .get_mut()
            .unwrap()
            .set_duration(gst::ClockTime::from_nseconds(1_000_000_000 / 15));
        Ok(buffer)
    }
    fn cursor(&self, pixels: &mut [u8], drawable: u32, x: i16, y: i16, width: u16, height: u16) {
        let Ok(cookie) = self.conn.xfixes_get_cursor_image() else {
            return;
        };
        let Ok(cursor) = cookie.reply() else { return };
        let (origin_x, origin_y) = if drawable == self.root {
            (i32::from(x), i32::from(y))
        } else {
            let Ok(cookie) = self.conn.translate_coordinates(drawable, self.root, 0, 0) else {
                return;
            };
            let Ok(position) = cookie.reply() else { return };
            (i32::from(position.dst_x), i32::from(position.dst_y))
        };
        let left = i32::from(cursor.x) - i32::from(cursor.xhot) - origin_x;
        let top = i32::from(cursor.y) - i32::from(cursor.yhot) - origin_y;
        for cy in 0..cursor.height as usize {
            for cx in 0..cursor.width as usize {
                let (dx, dy) = (left + cx as i32, top + cy as i32);
                if dx < 0 || dy < 0 || dx >= i32::from(width) || dy >= i32::from(height) {
                    continue;
                }
                let color = cursor.cursor_image[cy * cursor.width as usize + cx];
                let alpha = color >> 24;
                let index = (dy as usize * width as usize + dx as usize) * 4;
                for channel in 0..3 {
                    let value = (color >> (channel * 8)) & 255;
                    pixels[index + channel] = ((value
                        + u32::from(pixels[index + channel]) * (255 - alpha) / 255)
                        .min(255)) as u8;
                }
            }
        }
    }
}
fn caps(width: u16, height: u16) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", "BGRx")
        .field("width", i32::from(width))
        .field("height", i32::from(height))
        .field("framerate", gst::Fraction::new(15, 1))
        .build()
}
pub fn attach(source: gst::Element, selected: CaptureSourceInfo) -> Result<(), String> {
    let src = source
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| "Fonte de captura inválida")?;
    let capture = Capture::new(selected)?;
    src.set_caps(Some(&caps(capture.dimensions.0, capture.dimensions.1)));
    let capture = Mutex::new(capture);
    src.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |src, _| match capture.lock().unwrap().frame(src) {
                Ok(buffer) => {
                    let _ = src.push_buffer(buffer);
                }
                Err(error) => {
                    gst::element_error!(
                        src,
                        gst::ResourceError::Read,
                        ["Captura X11 encerrada: {}", error]
                    );
                    let _ = src.end_of_stream();
                }
            })
            .build(),
    );
    Ok(())
}
