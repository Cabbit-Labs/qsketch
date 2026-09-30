//! Windows Explorer thumbnail provider for `.qsk` documents.
//!
//! A `.qsk` is a ZIP with a flattened `preview.png` inside (see
//! `docs/FILE_FORMAT.md`). Explorer hands this in-process COM server the
//! file as an `IStream`; the provider reads only the ZIP's central directory
//! and that one entry, decodes the PNG, scales it to the requested size and
//! returns a premultiplied 32-bit DIB, which is what the shell draws as the
//! file's icon in every view that shows thumbnails.
//!
//! The installer registers the provider machine-wide (HKLM) for the `.qsk`
//! extension. `regsvr32 qsketch_thumb.dll` registers it for the current user
//! only (HKCU, no elevation needed), which is what the portable ZIP and the
//! Preferences button use; `regsvr32 /u` removes that again.
#![cfg(windows)]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::{implement, Error, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{
    IClassFactory, IClassFactory_Impl, IStream, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{
    IThumbnailProvider, IThumbnailProvider_Impl, SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, WTSAT_ARGB,
    WTS_ALPHATYPE,
};

/// Class id of the provider: registered under `CLSID\{…}` and named by the
/// `.qsk` extension's `ShellEx\{e357fccd-…}` (the IThumbnailProvider
/// handler category) key. Keep in sync with `installer/qsketch.nsi`.
pub const CLSID: GUID = GUID::from_u128(0x9b1f_2a6e_5c3d_4e7a_8f41_2d6c_0b7e_3a55);
const CLSID_TEXT: &str = "{9B1F2A6E-5C3D-4E7A-8F41-2D6C0B7E3A55}";
const SHELLEX_THUMBNAIL: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";
const PREVIEW_ENTRY: &str = "preview.png";
const DESCRIPTION: &str = "qsketch Thumbnail Provider";

/// Live COM objects, so `DllCanUnloadNow` can answer honestly.
static OBJECTS: AtomicUsize = AtomicUsize::new(0);

struct Counted;

impl Counted {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// The provider

#[implement(IInitializeWithStream, IThumbnailProvider)]
struct Provider {
    stream: RefCell<Option<IStream>>,
    _count: Counted,
}

impl Provider {
    fn new() -> Self {
        Self { stream: RefCell::new(None), _count: Counted::new() }
    }
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, pstream: Ref<IStream>, _grfmode: u32) -> Result<()> {
        let stream = pstream.ok()?.clone();
        *self.stream.borrow_mut() = Some(stream);
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, pdwalpha: *mut WTS_ALPHATYPE) -> Result<()> {
        if phbmp.is_null() || pdwalpha.is_null() {
            return Err(E_POINTER.into());
        }
        let stream = self.stream.borrow().clone().ok_or_else(|| Error::from(E_FAIL))?;
        let (w, h, premul) = read_preview(stream).map_err(|_| Error::from(E_FAIL))?;
        let (tw, th, px) = fit_into(w, h, &premul, cx.max(1));
        let hbmp = unsafe { dib_from_premultiplied(tw, th, &px) }?;
        unsafe {
            *phbmp = hbmp;
            *pdwalpha = WTSAT_ARGB;
        }
        Ok(())
    }
}

/// `std::io` view of the shell's `IStream`, so the `zip` crate can seek to
/// the central directory and read just the preview entry.
struct StreamReader(IStream);

impl Read for StreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        let hr = unsafe { self.0.Read(buf.as_mut_ptr().cast::<c_void>(), buf.len() as u32, Some(&mut n)) };
        if hr.is_ok() || hr == S_FALSE {
            Ok(n as usize)
        } else {
            Err(std::io::Error::other(format!("IStream::Read failed: {hr:?}")))
        }
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let (offset, origin) = match pos {
            SeekFrom::Start(o) => (o as i64, STREAM_SEEK_SET),
            SeekFrom::Current(o) => (o, STREAM_SEEK_CUR),
            SeekFrom::End(o) => (o, STREAM_SEEK_END),
        };
        let mut at = 0u64;
        unsafe { self.0.Seek(offset, origin, Some(&mut at)) }
            .map_err(|e| std::io::Error::other(format!("IStream::Seek failed: {e}")))?;
        Ok(at)
    }
}

type BoxError = Box<dyn std::error::Error>;

/// The document's stored preview as premultiplied RGBA.
fn read_preview(stream: IStream) -> std::result::Result<(u32, u32, Vec<u8>), BoxError> {
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(StreamReader(stream)))?;
    let mut bytes = Vec::new();
    zip.by_name(PREVIEW_ENTRY)?.read_to_end(&mut bytes)?;
    let (w, h, mut rgba) = decode_png(&bytes)?;
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Ok((w, h, rgba))
}

fn decode_png(bytes: &[u8]) -> std::result::Result<(u32, u32, Vec<u8>), BoxError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("preview too large")?];
    let info = reader.next_frame(&mut buf)?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Indexed => return Err("indexed preview was not expanded".into()),
    };
    if rgba.len() != (info.width as usize) * (info.height as usize) * 4 {
        return Err("preview size mismatch".into());
    }
    Ok((info.width, info.height, rgba))
}

/// Shrink (box filter, area average) so the longer side is at most `cx`.
/// Smaller pictures are returned as they are; the shell centres them.
fn fit_into(w: u32, h: u32, px: &[u8], cx: u32) -> (u32, u32, Vec<u8>) {
    let longest = w.max(h);
    if longest <= cx || w == 0 || h == 0 {
        return (w, h, px.to_vec());
    }
    let scale = cx as f64 / longest as f64;
    let tw = ((w as f64 * scale).round() as u32).max(1);
    let th = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; tw as usize * th as usize * 4];
    let span = |t: u32, n: u32, total: u32| -> (u32, u32) {
        let a = (t as u64 * total as u64 / n as u64) as u32;
        let b = (((t + 1) as u64 * total as u64 / n as u64) as u32).clamp(a + 1, total);
        (a, b)
    };
    for ty in 0..th {
        let (y0, y1) = span(ty, th, h);
        for tx in 0..tw {
            let (x0, x1) = span(tx, tw, w);
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                let row = (y as usize * w as usize + x0 as usize) * 4;
                for p in px[row..row + (x1 - x0) as usize * 4].chunks_exact(4) {
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += *v as u64;
                    }
                }
            }
            let n = ((y1 - y0) as u64) * ((x1 - x0) as u64);
            let o = (ty as usize * tw as usize + tx as usize) * 4;
            for (d, a) in out[o..o + 4].iter_mut().zip(acc) {
                *d = (a / n) as u8;
            }
        }
    }
    (tw, th, out)
}

/// A top-down 32-bit DIB holding premultiplied BGRA, as `WTSAT_ARGB` wants.
unsafe fn dib_from_premultiplied(w: u32, h: u32, premul: &[u8]) -> Result<HBITMAP> {
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: -(h as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let hbmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    if bits.is_null() {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        return Err(E_FAIL.into());
    }
    let out = std::slice::from_raw_parts_mut(bits.cast::<u8>(), w as usize * h as usize * 4);
    for (d, s) in out.chunks_exact_mut(4).zip(premul.chunks_exact(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
    Ok(hbmp)
}

// ---------------------------------------------------------------------------
// Class factory and DLL exports

#[implement(IClassFactory)]
struct Factory {
    _count: Counted,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, punkouter: Ref<IUnknown>, riid: *const GUID, ppvobject: *mut *mut c_void) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let unknown: IUnknown = Provider::new().into();
        unsafe { unknown.query(riid, ppvobject) }.ok()
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    if *rclsid != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory { _count: Counted::new() }.into();
    factory.query(riid, ppv)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Per-user registration (HKCU), so `regsvr32` works without elevation.
#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register(true) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    match register(false) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

fn register(on: bool) -> Result<()> {
    use windows_registry::CURRENT_USER;
    let classes = "Software\\Classes";
    let clsid_key = format!("{classes}\\CLSID\\{CLSID_TEXT}");
    let ext_key = format!("{classes}\\.qsk\\ShellEx\\{SHELLEX_THUMBNAIL}");
    if on {
        let path = module_path()?;
        CURRENT_USER.create(&clsid_key)?.set_string("", DESCRIPTION)?;
        let inproc = CURRENT_USER.create(format!("{clsid_key}\\InprocServer32"))?;
        inproc.set_string("", &path)?;
        inproc.set_string("ThreadingModel", "Apartment")?;
        CURRENT_USER.create(&ext_key)?.set_string("", CLSID_TEXT)?;
    } else {
        let _ = CURRENT_USER.remove_tree(&ext_key);
        let _ = CURRENT_USER.remove_tree(&clsid_key);
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

/// Full path of this DLL, for `InprocServer32`.
fn module_path() -> Result<String> {
    const FROM_ADDRESS: u32 = 0x0000_0004;
    const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
    let mut module = HMODULE::default();
    let anchor = (DllCanUnloadNow as extern "system" fn() -> HRESULT as usize) as *const u16;
    unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, PCWSTR(anchor), &mut module) }?;
    let mut buf = vec![0u16; 32 * 1024];
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 {
        return Err(E_FAIL.into());
    }
    Ok(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(test)]
mod tests {
    use super::fit_into;

    #[test]
    fn fit_shrinks_to_longest_side() {
        let (w, h) = (40u32, 20u32);
        let px = vec![255u8; (w * h * 4) as usize];
        let (tw, th, out) = fit_into(w, h, &px, 10);
        assert_eq!((tw, th), (10, 5));
        assert_eq!(out.len(), 10 * 5 * 4);
        assert!(out.iter().all(|&v| v == 255));
    }

    #[test]
    fn fit_keeps_small_pictures() {
        let px = vec![7u8; 4 * 4 * 4];
        let (tw, th, out) = fit_into(4, 4, &px, 256);
        assert_eq!((tw, th), (4, 4));
        assert_eq!(out, px);
    }
}
