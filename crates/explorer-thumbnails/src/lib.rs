//! Mangoo thumbnails in Windows Explorer.
//!
//! A COM in-process server implementing `IThumbnailProvider` +
//! `IInitializeWithStream`, registered per user (HKCU, no admin) by the image
//! viewer's "show thumbnails in File Explorer" setting. Explorer runs it in
//! its isolated surrogate (dllhost) with the file as a stream; we read at
//! most [`render::READ_LIMIT`] bytes, render a thumbnail with
//! [`render::render`], and hand back an HBITMAP.
//!
//! Registration (written by the Desktop host, not by this DLL):
//!
//! ```text
//! HKCU\Software\Classes\CLSID\{CLSID}\InprocServer32  (default) = <dll path>
//!                                                     ThreadingModel = Apartment
//! HKCU\Software\Classes\.<ext>\ShellEx\{e357fccd-a995-4576-b01f-234630154e96}  (default) = {CLSID}
//! ```

pub mod render;

/// The provider's CLSID; the Desktop host registers exactly this value.
pub const CLSID_TEXT: &str = "{6F3C2B7A-41E8-4C5D-9A2E-8B1D7C0E5A93}";

#[cfg(windows)]
mod com {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::core::{implement, Interface, GUID, HRESULT};
    use windows::Win32::Foundation::{
        BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOINTERFACE, E_POINTER,
        HINSTANCE, HWND, S_FALSE, S_OK,
    };
    use windows::Win32::Graphics::Gdi::{
        CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::{
        IClassFactory, IClassFactory_Impl, IStream, STREAM_SEEK_SET,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IInitializeWithStream, IInitializeWithStream_Impl,
    };
    use windows::Win32::UI::Shell::{
        IThumbnailProvider, IThumbnailProvider_Impl, SHChangeNotify, SHCNE_ASSOCCHANGED,
        SHCNF_IDLIST, WTSAT_ARGB, WTSAT_RGB, WTS_ALPHATYPE,
    };

    use crate::render;

    pub const CLSID: GUID = GUID::from_u128(0x6F3C2B7A_41E8_4C5D_9A2E_8B1D7C0E5A93);
    static LOCKS: AtomicU32 = AtomicU32::new(0);
    /// Live COM objects (factories + providers). The DLL may unload only
    /// when no object and no server lock remains.
    static OBJECTS: AtomicU32 = AtomicU32::new(0);

    /// Counts itself into [`OBJECTS`] for the lifetime of its owner.
    struct Alive;
    impl Alive {
        fn new() -> Self {
            OBJECTS.fetch_add(1, Ordering::SeqCst);
            Alive
        }
    }
    impl Drop for Alive {
        fn drop(&mut self) {
            OBJECTS.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[implement(IThumbnailProvider, IInitializeWithStream)]
    struct Provider {
        stream: RefCell<Option<IStream>>,
        _alive: Alive,
    }

    /// Read the stream in growing steps (4 MB, 16 MB, ... up to
    /// [`render::READ_LIMIT`]) and stop as soon as the prefix renders: camera
    /// previews sit near the start, so a NAS-hosted RAW costs a few MB, not
    /// the whole file. Plain images (WebP, PSD composite) render only once
    /// complete, which the final step guarantees.
    fn read_and_render(stream: &IStream, cx: u32) -> windows::core::Result<render::Thumbnail> {
        let mut bytes = Vec::new();
        let mut chunk = vec![0u8; 1 << 20];
        let mut goal = 4usize << 20;
        let mut finished = false;
        unsafe { stream.Seek(0, STREAM_SEEK_SET, None)? };
        loop {
            while !finished && bytes.len() < goal {
                let mut read = 0u32;
                let hr = unsafe {
                    stream.Read(
                        chunk.as_mut_ptr() as *mut c_void,
                        chunk.len() as u32,
                        Some(&mut read),
                    )
                };
                if hr.is_err() {
                    return Err(hr.into());
                }
                bytes.extend_from_slice(&chunk[..read as usize]);
                if read == 0 || hr == S_FALSE || bytes.len() >= render::READ_LIMIT {
                    finished = true;
                }
            }
            let prefix = &bytes;
            let rendered = std::panic::catch_unwind(|| {
                if finished {
                    render::render(prefix, cx)
                } else {
                    render::render_preview(prefix, cx)
                }
            })
            .ok()
            .flatten();
            if let Some(thumb) = rendered {
                return Ok(thumb);
            }
            if finished {
                return Err(E_FAIL.into());
            }
            goal = (goal * 4).min(render::READ_LIMIT);
        }
    }

    impl IInitializeWithStream_Impl for Provider {
        fn Initialize(&self, stream: Option<&IStream>, _mode: u32) -> windows::core::Result<()> {
            let stream = stream.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
            *self.stream.borrow_mut() = Some(stream.clone());
            Ok(())
        }
    }

    impl IThumbnailProvider_Impl for Provider {
        fn GetThumbnail(
            &self,
            cx: u32,
            phbmp: *mut HBITMAP,
            pdwalpha: *mut WTS_ALPHATYPE,
        ) -> windows::core::Result<()> {
            if phbmp.is_null() || pdwalpha.is_null() {
                return Err(E_POINTER.into());
            }
            let stream = self
                .stream
                .borrow_mut()
                .take()
                .ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            // Rendering is panic-contained: a malformed file must never take
            // Explorer's surrogate down.
            let thumb = read_and_render(&stream, cx)?;
            unsafe {
                let header = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: thumb.width as i32,
                    // Negative height: top-down rows, matching our buffer.
                    biHeight: -(thumb.height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                };
                let info = BITMAPINFO {
                    bmiHeader: header,
                    ..Default::default()
                };
                let mut bits: *mut c_void = std::ptr::null_mut();
                let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
                if bits.is_null() {
                    return Err(E_FAIL.into());
                }
                std::ptr::copy_nonoverlapping(
                    thumb.bgra.as_ptr(),
                    bits as *mut u8,
                    thumb.bgra.len(),
                );
                *phbmp = bitmap;
                *pdwalpha = if thumb.has_alpha {
                    WTSAT_ARGB
                } else {
                    WTSAT_RGB
                };
            }
            Ok(())
        }
    }

    #[implement(IClassFactory)]
    struct Factory {
        _alive: Alive,
    }

    impl IClassFactory_Impl for Factory {
        fn CreateInstance(
            &self,
            outer: Option<&windows::core::IUnknown>,
            riid: *const GUID,
            object: *mut *mut c_void,
        ) -> windows::core::Result<()> {
            if object.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *object = std::ptr::null_mut() };
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let provider: IThumbnailProvider = Provider {
                stream: RefCell::new(None),
                _alive: Alive::new(),
            }
            .into();
            unsafe { provider.query(riid, object).ok() }
        }

        fn LockServer(&self, lock: BOOL) -> windows::core::Result<()> {
            if lock.as_bool() {
                LOCKS.fetch_add(1, Ordering::SeqCst);
            } else {
                // Saturating: an unbalanced unlock must not wrap to "locked".
                let _ =
                    LOCKS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
            }
            Ok(())
        }
    }

    #[no_mangle]
    extern "system" fn DllGetClassObject(
        rclsid: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        unsafe {
            *ppv = std::ptr::null_mut();
            if *rclsid != CLSID {
                return CLASS_E_CLASSNOTAVAILABLE;
            }
            let factory: IClassFactory = Factory {
                _alive: Alive::new(),
            }
            .into();
            let hr = factory.query(riid, ppv);
            if hr.is_err() {
                E_NOINTERFACE
            } else {
                hr
            }
        }
    }

    /// `rundll32 "<dll>",Refresh` — tell Explorer that thumbnail handlers
    /// changed, so open windows pick the new registration up.
    #[no_mangle]
    extern "system" fn Refresh(_hwnd: HWND, _instance: HINSTANCE, _command: *const u8, _show: i32) {
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }

    #[no_mangle]
    extern "system" fn DllCanUnloadNow() -> HRESULT {
        if LOCKS.load(Ordering::SeqCst) == 0 && OBJECTS.load(Ordering::SeqCst) == 0 {
            S_OK
        } else {
            S_FALSE
        }
    }
}
