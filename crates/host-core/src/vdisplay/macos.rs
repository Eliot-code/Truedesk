//! Virtual displays through CoreGraphics' `CGVirtualDisplay` (macOS 11+).
//!
//! The API is private; DeskPad (MIT) and BetterDisplay use it too. The
//! interface, from DeskPad's `CGVirtualDisplayPrivate.h`:
//!
//! - `CGVirtualDisplayDescriptor`: `queue`, `name`, `maxPixelsWide`,
//!   `maxPixelsHigh`, `sizeInMillimeters`, `vendorID`, `productID`,
//!   `serialNum`
//! - `CGVirtualDisplayMode`: `initWithWidth:height:refreshRate:`
//! - `CGVirtualDisplaySettings`: `modes`, `hiDPI`
//! - `CGVirtualDisplay`: `initWithDescriptor:`, `applySettings:`,
//!   `displayID`
//!
//! Every class and selector is looked up at run time first; if one is
//! missing, the backend reports itself unsupported and never sends it. A
//! display lives exactly as long as its `CGVirtualDisplay` object, so
//! releasing the object removes it, and so does the process exiting.
//! The objects stay on one thread that owns them.

use super::{AddedDisplay, DisplaySpec, VirtualDisplayError, VirtualDisplays};
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel, Encode, Encoding};
use objc2_foundation::{NSArray, NSString};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

// SAFETY: matches CoreGraphics' CGSize layout and type encoding.
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

extern "C" {
    // libdispatch, part of libSystem.
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
}

/// The private classes, present and answering every selector we send.
#[derive(Clone, Copy)]
struct Api {
    descriptor: &'static AnyClass,
    settings: &'static AnyClass,
    mode: &'static AnyClass,
    display: &'static AnyClass,
    /// The descriptor's queue setter; older releases name it differently.
    set_queue: Option<Sel>,
}

impl Api {
    fn load() -> Option<Self> {
        let descriptor = AnyClass::get(c"CGVirtualDisplayDescriptor")?;
        let settings = AnyClass::get(c"CGVirtualDisplaySettings")?;
        let mode = AnyClass::get(c"CGVirtualDisplayMode")?;
        let display = AnyClass::get(c"CGVirtualDisplay")?;
        let has = |class: &AnyClass, sels: &[Sel]| sels.iter().all(|&sel| class.instance_method(sel).is_some());
        let complete = has(descriptor, &[
            sel!(setName:), sel!(setMaxPixelsWide:), sel!(setMaxPixelsHigh:), sel!(setSizeInMillimeters:),
            sel!(setVendorID:), sel!(setProductID:), sel!(setSerialNum:),
        ]) && has(settings, &[sel!(setModes:), sel!(setHiDPI:)])
            && has(mode, &[sel!(initWithWidth:height:refreshRate:)])
            && has(display, &[sel!(initWithDescriptor:), sel!(applySettings:), sel!(displayID)]);
        if !complete {
            log::warn!("CGVirtualDisplay is present but not as expected; virtual displays are off");
            return None;
        }
        let set_queue = [sel!(setQueue:), sel!(setDispatchQueue:)].into_iter().find(|&sel| descriptor.instance_method(sel).is_some());
        Some(Self { descriptor, settings, mode, display, set_queue })
    }
}

enum Request {
    Create(DisplaySpec, mpsc::Sender<Result<u32, VirtualDisplayError>>),
    Remove(u32),
}

pub struct CgVirtualDisplays {
    api: Option<Api>,
    /// The thread holding the displays, started on first use.
    requests: Mutex<Option<mpsc::Sender<Request>>>,
    owned: Mutex<Vec<u32>>,
}

impl CgVirtualDisplays {
    pub fn new() -> Self {
        Self { api: Api::load(), requests: Mutex::new(None), owned: Mutex::new(Vec::new()) }
    }

    fn send(&self, request: Request) -> bool {
        let Some(api) = self.api else { return false };
        let mut requests = self.requests.lock().unwrap_or_else(|poison| poison.into_inner());
        if requests.is_none() {
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("opendesk-virtual-display".into())
                .spawn(move || serve(api, &rx));
            if let Err(error) = spawned {
                log::warn!("could not start the virtual display thread: {error}");
                return false;
            }
            *requests = Some(tx);
        }
        requests.as_ref().is_some_and(|tx| tx.send(request).is_ok())
    }
}

impl VirtualDisplays for CgVirtualDisplays {
    fn supported(&self) -> bool { self.api.is_some() }

    fn create(&self, spec: &DisplaySpec) -> Result<AddedDisplay, VirtualDisplayError> {
        if self.api.is_none() {
            return Err(VirtualDisplayError::Unsupported);
        }
        let (reply, result) = mpsc::channel();
        if !self.send(Request::Create(*spec, reply)) {
            return Err(VirtualDisplayError::Failed("the virtual display thread is gone".into()));
        }
        let id = result.recv().unwrap_or_else(|_| Err(VirtualDisplayError::Failed("the virtual display thread stopped".into())))?;
        self.owned.lock().unwrap_or_else(|poison| poison.into_inner()).push(id);
        log::info!("added virtual display {id} ({}x{}{})", spec.width, spec.height, if spec.hidpi { " HiDPI" } else { "" });
        // macOS makes the mode asked for.
        Ok(AddedDisplay { id, width: spec.width, height: spec.height })
    }

    fn remove(&self, id: u32) -> Result<(), VirtualDisplayError> {
        let mut owned = self.owned.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(index) = owned.iter().position(|&owned| owned == id) else { return Ok(()) };
        owned.remove(index);
        drop(owned);
        if self.send(Request::Remove(id)) {
            log::info!("removed virtual display {id}");
        }
        Ok(())
    }

    fn owned(&self) -> Vec<u32> { self.owned.lock().unwrap_or_else(|poison| poison.into_inner()).clone() }
}

/// Body of the display thread.
fn serve(api: Api, requests: &mpsc::Receiver<Request>) {
    let mut displays: HashMap<u32, Retained<AnyObject>> = HashMap::new();
    while let Ok(request) = requests.recv() {
        match request {
            Request::Create(spec, reply) => {
                let created = autoreleasepool(|_| create(api, &spec));
                let _ = reply.send(created.map(|(id, display)| {
                    displays.insert(id, display);
                    id
                }));
            }
            // Releasing the object takes the display away.
            Request::Remove(id) => autoreleasepool(|_| drop(displays.remove(&id))),
        }
    }
}

/// Product ids rotate through a small pool: macOS keeps a released
/// display's id busy for several seconds, and a new display that reuses
/// it right away never comes online. It also remembers each id's
/// arrangement, so the pool stays small.
const PRODUCT_POOL: u32 = 16;
static NEXT_PRODUCT: AtomicU32 = AtomicU32::new(0);
/// Serial numbers keep the displays of one run apart.
static SERIAL: AtomicU32 = AtomicU32::new(1);

const VENDOR_ID: u32 = 0x4F44; // "OD"
const PRODUCT_BASE: u32 = 0x0100;
/// Pixel density reported to macOS, in points per inch.
const POINTS_PER_INCH: f64 = 110.0;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}

/// The displays macOS has online, the virtual ones included. Needs no
/// permission, unlike listing them for capture.
pub(crate) fn online_displays() -> Vec<u32> {
    let mut ids = [0u32; 32];
    let mut count = 0;
    // SAFETY: the buffer holds `max` ids and `count` is a valid out pointer.
    let error = unsafe { CGGetOnlineDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if error != 0 {
        return Vec::new();
    }
    ids[..(count as usize).min(ids.len())].to_vec()
}

fn create(api: Api, spec: &DisplaySpec) -> Result<(u32, Retained<AnyObject>), VirtualDisplayError> {
    let failed = |reason: &str| VirtualDisplayError::Failed(reason.into());
    let scale = if spec.hidpi { 2 } else { 1 };
    // The mode is in points; maxPixels bounds the backing store.
    let (points_w, points_h) = (spec.width / scale, spec.height / scale);
    let millimeters = |points: u32| f64::from(points) / POINTS_PER_INCH * 25.4;
    let name = NSString::from_str("OpenDesk Virtual Display");
    let product = PRODUCT_BASE + NEXT_PRODUCT.fetch_add(1, Ordering::Relaxed) % PRODUCT_POOL;
    // SAFETY: Api::load checked that every class exists and implements
    // each selector below; argument types match the private header.
    let display = unsafe {
        let descriptor: Option<Retained<AnyObject>> = msg_send![api.descriptor, new];
        let descriptor = descriptor.ok_or_else(|| failed("no descriptor"))?;
        if let Some(set_queue) = api.set_queue {
            let queue = dispatch_get_global_queue(0, 0);
            if set_queue == sel!(setQueue:) {
                let _: () = msg_send![&*descriptor, setQueue: queue];
            } else {
                let _: () = msg_send![&*descriptor, setDispatchQueue: queue];
            }
        }
        let _: () = msg_send![&*descriptor, setName: &*name];
        let _: () = msg_send![&*descriptor, setMaxPixelsWide: spec.width];
        let _: () = msg_send![&*descriptor, setMaxPixelsHigh: spec.height];
        let size = CGSize { width: millimeters(points_w), height: millimeters(points_h) };
        let _: () = msg_send![&*descriptor, setSizeInMillimeters: size];
        let _: () = msg_send![&*descriptor, setVendorID: VENDOR_ID];
        let _: () = msg_send![&*descriptor, setProductID: product];
        let _: () = msg_send![&*descriptor, setSerialNum: SERIAL.fetch_add(1, Ordering::Relaxed)];

        let display: Allocated<AnyObject> = msg_send![api.display, alloc];
        let display: Option<Retained<AnyObject>> = msg_send![display, initWithDescriptor: &*descriptor];
        let display = display.ok_or_else(|| failed("macOS declined to create the display"))?;

        let mode: Allocated<AnyObject> = msg_send![api.mode, alloc];
        let mode: Option<Retained<AnyObject>> =
            msg_send![mode, initWithWidth: points_w, height: points_h, refreshRate: f64::from(spec.refresh_hz)];
        let mode = mode.ok_or_else(|| failed("macOS declined the display mode"))?;
        let modes = NSArray::from_retained_slice(&[mode]);
        let settings: Option<Retained<AnyObject>> = msg_send![api.settings, new];
        let settings = settings.ok_or_else(|| failed("no settings"))?;
        let _: () = msg_send![&*settings, setHiDPI: u32::from(spec.hidpi)];
        let _: () = msg_send![&*settings, setModes: &*modes];
        let applied: Bool = msg_send![&*display, applySettings: &*settings];
        if !applied.as_bool() {
            return Err(failed("macOS rejected the display settings"));
        }
        display
    };
    // SAFETY: checked selector, no arguments.
    let id: u32 = unsafe { msg_send![&*display, displayID] };
    if id == 0 {
        return Err(failed("the display has no id"));
    }
    // Not waited for here: asking CoreGraphics for the display list while
    // the display is coming up keeps the next display from ever coming
    // online. The session waits for the capturer to list it instead.
    Ok((id, display))
}
