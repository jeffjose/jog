//! HEIC and AVIF through libheif, loaded at run time so that jog still
//! builds and runs, for everything else, on machines without it.
//!
//! libheif reads the container and hands the picture to whichever decoder
//! it was built with: libde265 for HEIC, dav1d or libaom for AVIF.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;
use std::sync::OnceLock;

use image::{DynamicImage, RgbImage, RgbaImage};
use libloading::Library;

use crate::decode::Result;

const LIBRARY_NAMES: &[&str] = &["libheif.so.1", "libheif.so"];

// Values from heif.h.
const COLORSPACE_RGB: c_int = 1;
const CHROMA_INTERLEAVED_RGB: c_int = 10;
const CHROMA_INTERLEAVED_RGBA: c_int = 11;
const CHANNEL_INTERLEAVED: c_int = 10;

type Context = *mut c_void;
type ImageHandle = *mut c_void;
type Image = *mut c_void;

/// What most calls return: `code` is zero when all went well.
#[repr(C)]
struct RawError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

/// The libheif functions jog calls.
struct Api {
    context_alloc: unsafe extern "C" fn() -> Context,
    context_free: unsafe extern "C" fn(Context),
    context_read_from_memory_without_copy: unsafe extern "C" fn(Context, *const c_void, usize, *const c_void) -> RawError,
    context_get_primary_image_handle: unsafe extern "C" fn(Context, *mut ImageHandle) -> RawError,
    image_handle_has_alpha_channel: unsafe extern "C" fn(ImageHandle) -> c_int,
    image_handle_release: unsafe extern "C" fn(ImageHandle),
    decode_image: unsafe extern "C" fn(ImageHandle, *mut Image, c_int, c_int, *const c_void) -> RawError,
    image_get_width: unsafe extern "C" fn(Image, c_int) -> c_int,
    image_get_height: unsafe extern "C" fn(Image, c_int) -> c_int,
    image_get_plane_readonly: unsafe extern "C" fn(Image, c_int, *mut c_int) -> *const u8,
    image_release: unsafe extern "C" fn(Image),
    /// Keeps the function pointers above valid.
    _library: Library,
}

impl Api {
    fn load(library: Library) -> std::result::Result<Api, libloading::Error> {
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: each symbol is given the signature its C header declares.
                *unsafe { library.get(concat!("heif_", $name, "\0").as_bytes())? }
            };
        }
        Ok(Api {
            context_alloc: symbol!("context_alloc"),
            context_free: symbol!("context_free"),
            context_read_from_memory_without_copy: symbol!("context_read_from_memory_without_copy"),
            context_get_primary_image_handle: symbol!("context_get_primary_image_handle"),
            image_handle_has_alpha_channel: symbol!("image_handle_has_alpha_channel"),
            image_handle_release: symbol!("image_handle_release"),
            decode_image: symbol!("decode_image"),
            image_get_width: symbol!("image_get_width"),
            image_get_height: symbol!("image_get_height"),
            image_get_plane_readonly: symbol!("image_get_plane_readonly"),
            image_release: symbol!("image_release"),
            _library: library,
        })
    }
}

/// The library, looked for once: `JOG_LIBHEIF` names one to use instead.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let chosen = std::env::var("JOG_LIBHEIF").ok();
        let names = chosen.iter().map(String::as_str).chain(LIBRARY_NAMES.iter().copied());
        // SAFETY: loading libheif runs only its ordinary library initialisers.
        names.filter_map(|name| unsafe { Library::new(name) }.ok()).find_map(|library| Api::load(library).ok())
    })
    .as_ref()
}

/// Whatever was allocated for one picture, freed when it goes.
struct Parts<'a> {
    api: &'a Api,
    context: Context,
    handle: ImageHandle,
    image: Image,
}

impl Drop for Parts<'_> {
    fn drop(&mut self) {
        // SAFETY: each pointer is null or came from libheif and is freed once.
        unsafe {
            if !self.image.is_null() {
                (self.api.image_release)(self.image);
            }
            if !self.handle.is_null() {
                (self.api.image_handle_release)(self.handle);
            }
            (self.api.context_free)(self.context);
        }
    }
}

fn check(error: RawError) -> Result<()> {
    if error.code == 0 {
        return Ok(());
    }
    // SAFETY: the message of an error is a NUL-terminated string that
    // libheif owns and keeps.
    let message = if error.message.is_null() { "".into() } else { unsafe { CStr::from_ptr(error.message) }.to_string_lossy() };
    Err(format!("libheif: {message} ({}.{})", error.code, error.subcode).into())
}

/// The main picture of a HEIC or AVIF file, upright: libheif has already
/// turned it the way the file says.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let api = api().ok_or("HEIC and AVIF need libheif, or an ffmpeg that reads them")?;
    // SAFETY: `bytes` outlives the context that reads from it, every
    // pointer is checked before use, and the plane is read within the
    // height and stride libheif reports for it.
    unsafe {
        let mut parts = Parts { api, context: (api.context_alloc)(), handle: ptr::null_mut(), image: ptr::null_mut() };
        if parts.context.is_null() {
            return Err("libheif: out of memory".into());
        }
        check((api.context_read_from_memory_without_copy)(parts.context, bytes.as_ptr().cast(), bytes.len(), ptr::null()))?;
        check((api.context_get_primary_image_handle)(parts.context, &mut parts.handle))?;
        let alpha = (api.image_handle_has_alpha_channel)(parts.handle) != 0;
        let (chroma, depth) = if alpha { (CHROMA_INTERLEAVED_RGBA, 4) } else { (CHROMA_INTERLEAVED_RGB, 3) };
        check((api.decode_image)(parts.handle, &mut parts.image, COLORSPACE_RGB, chroma, ptr::null()))?;

        let width = (api.image_get_width)(parts.image, CHANNEL_INTERLEAVED);
        let height = (api.image_get_height)(parts.image, CHANNEL_INTERLEAVED);
        let mut stride = 0;
        let plane = (api.image_get_plane_readonly)(parts.image, CHANNEL_INTERLEAVED, &mut stride);
        let row = usize::try_from(width)? * depth;
        if plane.is_null() || width <= 0 || height <= 0 || usize::try_from(stride)? < row {
            return Err("libheif: no pixels in the decoded picture".into());
        }
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            pixels.extend_from_slice(std::slice::from_raw_parts(plane.add(y * stride as usize), row));
        }
        let (width, height) = (width as u32, height as u32);
        let image = if alpha {
            RgbaImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgba8)
        } else {
            RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8)
        };
        Ok(image.expect("buffer matches dimensions"))
    }
}
