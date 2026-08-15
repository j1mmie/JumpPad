//! Softbuffer implementation using CoreGraphics.
use crate::backend_interface::*;
use crate::error::InitError;
use crate::{util, Rect, SoftBufferError};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker, Message};
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGImageComponentInfo, CGImagePixelFormatInfo,
};
use objc2_foundation::{
    ns_string, NSDictionary, NSKeyValueChangeKey, NSKeyValueChangeNewKey,
    NSKeyValueObservingOptions, NSNumber, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSString, NSValue,
};
use objc2_quartz_core::{kCAGravityTopLeft, CALayer, CATransaction};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};

use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::marker::PhantomData;
use std::mem::size_of;
use std::num::NonZeroU32;
use std::ops::Deref;
use std::ptr::{self, slice_from_raw_parts_mut, NonNull};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SoftbufferObserver"]
    #[ivars = SendCALayer]
    #[derive(Debug)]
    struct Observer;

    /// NSKeyValueObserving
    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            self.update(key_path, change);
        }
    }
);

impl Observer {
    fn new(layer: &CALayer) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SendCALayer(layer.retain()));
        unsafe { msg_send![super(this), init] }
    }

    fn update(
        &self,
        key_path: Option<&NSString>,
        change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
    ) {
        let layer = self.ivars();

        let change =
            change.expect("requested a change dictionary in `addObserver`, but none was provided");
        let new = change
            .objectForKey(unsafe { NSKeyValueChangeNewKey })
            .expect("requested change dictionary did not contain `NSKeyValueChangeNewKey`");

        // NOTE: Setting these values usually causes a quarter second animation to occur, which is
        // undesirable.
        //
        // However, since we're setting them inside an observer, there already is a transaction
        // ongoing, and as such we don't need to wrap this in a `CATransaction` ourselves.

        if key_path == Some(ns_string!("contentsScale")) {
            let new = new.downcast::<NSNumber>().unwrap();
            let scale_factor = new.as_cgfloat();

            // Set the scale factor of the layer to match the root layer when it changes (e.g. if
            // moved to a different monitor, or monitor settings changed).
            layer.setContentsScale(scale_factor);
        } else if key_path == Some(ns_string!("bounds")) {
            let new = new.downcast::<NSValue>().unwrap();
            let bounds = new.get_rect().expect("new bounds value was not CGRect");

            // Set `bounds` and `position` so that the new layer is inside the superlayer.
            //
            // This differs from just setting the `bounds`, as it also takes into account any
            // translation that the superlayer may have that we'd want to preserve.
            layer.setFrame(bounds);
        } else {
            panic!("unknown observed keypath {key_path:?}");
        }
    }
}

/// The most buffers kept alive for reuse at once.
///
/// Core Graphics owns at most the frame on screen plus the one being handed
/// over, so a steady stream of frames settles at two. The third is slack for
/// the moments Core Animation holds one a little longer.
const MAX_POOLED_BUFFERS: usize = 3;

/// Whether `SOFTBUFFER_TRACE_AGE` asked for the ages `take` hands out.
///
/// `age` decides whether the caller can do damage tracking at all, and nothing
/// outside this crate can observe it - a pool that silently misses every frame
/// looks exactly like a working one from the application side, right down to
/// the memory profile. Printing it is the only way to tell the two apart.
fn age_tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SOFTBUFFER_TRACE_AGE").is_some())
}

/// How many frames the tracing below reports before going quiet, overridable
/// with `SOFTBUFFER_TRACE_FRAMES`.
///
/// The default is enough to watch the pool warm up and settle. Raise it when
/// whatever is being measured only begins after some setup: dragging the
/// window to the size under test burns frames before the steady state starts,
/// and a hand-drag burns far more of them than a keyboard-driven resize.
fn traced_frames() -> usize {
    const DEFAULT_TRACED_FRAMES: usize = 24;
    static FRAMES: OnceLock<usize> = OnceLock::new();

    *FRAMES.get_or_init(|| {
        std::env::var("SOFTBUFFER_TRACE_FRAMES")
            .ok()
            .and_then(|frames| frames.parse().ok())
            .unwrap_or(DEFAULT_TRACED_FRAMES)
    })
}

/// Prints the age handed out, for as many frames as `traced_frames` allows.
fn trace_age(age: u8, available: usize, held: usize) {
    static FRAMES: AtomicUsize = AtomicUsize::new(0);

    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    if frame < traced_frames() {
        eprintln!("softbuffer: frame {frame} age {age}, {available} pooled, {held} held by CG");
    }
}

/// Prints how long the caller spent drawing, for the frames `trace_age` covers.
///
/// Measured from `buffer_mut` returning to `present` being entered, which for
/// `iced_tiny_skia` is its damage diff plus `renderer.draw`. That is the half
/// of a frame `age` is supposed to shrink: a caret blink damages a 1px-wide
/// quad, so a number here that tracks window area rather than the caret means
/// the damage rectangles are still coming back full-window.
fn trace_draw(elapsed: std::time::Duration) {
    static FRAMES: AtomicUsize = AtomicUsize::new(0);

    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    if frame < traced_frames() {
        eprintln!(
            "softbuffer: frame {frame} draw {:.2}ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

/// Prints how long `present` took, for the same frames `trace_age` covers.
///
/// This is what splits the per-frame cost in two. The caller has already
/// rasterized into the buffer by the time `present` runs, so whatever is spent
/// here is the cost of handing a finished frame to Core Animation - and unlike
/// rasterization, no amount of damage tracking reduces it. A large number here
/// means `age` is doing its job and the bottleneck is somewhere it cannot
/// reach.
fn trace_present(elapsed: std::time::Duration) {
    static FRAMES: AtomicUsize = AtomicUsize::new(0);

    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    if frame < traced_frames() {
        eprintln!(
            "softbuffer: frame {frame} present {:.2}ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

/// A buffer that Core Graphics has finished with, and the present its pixels
/// came from.
struct PooledBuffer {
    pixels: Box<[u32]>,
    presented_at: u64,
}

/// Frame buffers kept alive across presents so `age` can report a real number.
///
/// Without this, `buffer_mut` allocated a fresh zeroed `Vec<u32>` every frame -
/// 8MB at a Retina window size, mapped and page-faulted in from scratch each
/// time - and `age` could only answer `0`, which tells the caller the contents
/// are undefined and forces it to repaint the whole window. Recycling the
/// allocations is what makes an honest `age` possible, and the honest `age` is
/// what lets a caller repaint only the region that actually changed.
struct BufferPool {
    state: Mutex<PoolState>,
}

struct PoolState {
    /// Buffers Core Graphics has released back to us, ready to draw into.
    available: Vec<PooledBuffer>,
    /// Buffers Core Graphics still owns, keyed by the address of their pixels.
    /// `release` is handed that address and nothing else, so the present each
    /// buffer was shown at has to be parked here until it comes back.
    held_by_core_graphics: HashMap<usize, u64>,
    /// Presents completed on this surface. An `age` is the distance between
    /// this and a buffer's `presented_at`.
    presents: u64,
    /// The element count every pooled buffer has. A resize changes it, which
    /// retires everything allocated at the old size.
    len: usize,
}

impl fmt::Debug for BufferPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferPool").finish_non_exhaustive()
    }
}

impl BufferPool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PoolState {
                available: Vec::new(),
                held_by_core_graphics: HashMap::new(),
                presents: 0,
                len: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A buffer to draw the next frame into, and how many presents ago its
    /// pixels were last on screen - `0` when they are undefined.
    fn take(&self, len: usize) -> (util::PixelBuffer, u8) {
        let mut state = self.lock();

        if state.len != len {
            state.len = len;
            state.available.clear();
        }

        // The most recently presented buffer is the one whose pixels are
        // closest to what is on screen, so it leaves the caller the smallest
        // region to repaint.
        let newest = state
            .available
            .iter()
            .enumerate()
            .max_by_key(|(_, pooled)| pooled.presented_at)
            .map(|(index, _)| index);

        let (pixels, age) = match newest {
            Some(index) => {
                let pooled = state.available.swap_remove(index);
                // An age too small is the one dangerous answer: it points the
                // caller at a more recent frame than this buffer actually
                // holds, and it skips the difference. So an age that doesn't
                // fit becomes `0` - undefined contents, repaint everything -
                // rather than a clamp to `u8::MAX`. Only reachable if a buffer
                // sits unused for 255 presents while others rotate, which
                // `MAX_POOLED_BUFFERS` makes vanishingly unlikely.
                let age =
                    u8::try_from(state.presents.saturating_sub(pooled.presented_at)).unwrap_or(0);

                (util::PixelBuffer(pooled.pixels.into_vec()), age)
            }
            None => (util::PixelBuffer(vec![0; len]), 0),
        };

        if age_tracing_enabled() {
            trace_age(
                age,
                state.available.len(),
                state.held_by_core_graphics.len(),
            );
        }

        (pixels, age)
    }

    /// Records that `pixels` is about to be handed to Core Graphics.
    fn mark_presented(&self, pixels: usize) {
        let mut state = self.lock();
        let presented_at = state.presents;
        state.held_by_core_graphics.insert(pixels, presented_at);
        state.presents += 1;
    }

    /// Takes a buffer back once Core Graphics has released it.
    fn reclaim(&self, pixels: Box<[u32]>) {
        let mut state = self.lock();

        let Some(presented_at) = state
            .held_by_core_graphics
            .remove(&(pixels.as_ptr() as usize))
        else {
            return;
        };

        // Allocated before a resize, or more slack than a rotation needs.
        if pixels.len() != state.len || state.available.len() >= MAX_POOLED_BUFFERS {
            return;
        }

        state.available.push(PooledBuffer {
            pixels,
            presented_at,
        });
    }
}

#[derive(Debug)]
pub struct CGImpl<D, W> {
    /// Our layer.
    layer: SendCALayer,
    /// The layer that our layer was created from.
    ///
    /// Can also be retrieved from `layer.superlayer()`.
    root_layer: SendCALayer,
    observer: Retained<Observer>,
    color_space: CFRetained<CGColorSpace>,
    /// The width of the underlying buffer.
    width: usize,
    /// The height of the underlying buffer.
    height: usize,
    /// Frame buffers recycled across presents - see `BufferPool`.
    pool: Arc<BufferPool>,
    window_handle: W,
    _display: PhantomData<D>,
}

impl<D, W> Drop for CGImpl<D, W> {
    fn drop(&mut self) {
        // SAFETY: Registered in `new`, must be removed before the observer is deallocated.
        unsafe {
            self.root_layer
                .removeObserver_forKeyPath(&self.observer, ns_string!("contentsScale"));
            self.root_layer
                .removeObserver_forKeyPath(&self.observer, ns_string!("bounds"));
        }
    }
}

impl<D: HasDisplayHandle, W: HasWindowHandle> SurfaceInterface<D, W> for CGImpl<D, W> {
    type Context = D;
    type Buffer<'a>
        = BufferImpl<'a, D, W>
    where
        Self: 'a;

    fn new(window_src: W, _display: &D) -> Result<Self, InitError<W>> {
        // `NSView`/`UIView` can only be accessed from the main thread.
        let _mtm = MainThreadMarker::new().ok_or(SoftBufferError::PlatformError(
            Some("can only access Core Graphics handles from the main thread".to_string()),
            None,
        ))?;

        let root_layer = match window_src.window_handle()?.as_raw() {
            RawWindowHandle::AppKit(handle) => {
                // SAFETY: The pointer came from `WindowHandle`, which ensures that the
                // `AppKitWindowHandle` contains a valid pointer to an `NSView`.
                //
                // We use `NSObject` here to avoid importing `objc2-app-kit`.
                let view: &NSObject = unsafe { handle.ns_view.cast().as_ref() };

                // Force the view to become layer backed
                let _: () = unsafe { msg_send![view, setWantsLayer: Bool::YES] };

                // SAFETY: `-[NSView layer]` returns an optional `CALayer`
                let layer: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
                layer.expect("failed making the view layer-backed")
            }
            RawWindowHandle::UiKit(handle) => {
                // SAFETY: The pointer came from `WindowHandle`, which ensures that the
                // `UiKitWindowHandle` contains a valid pointer to an `UIView`.
                //
                // We use `NSObject` here to avoid importing `objc2-ui-kit`.
                let view: &NSObject = unsafe { handle.ui_view.cast().as_ref() };

                // SAFETY: `-[UIView layer]` returns `CALayer`
                let layer: Retained<CALayer> = unsafe { msg_send![view, layer] };
                layer
            }
            _ => return Err(InitError::Unsupported(window_src)),
        };

        // Add a sublayer, to avoid interfering with the root layer, since setting the contents of
        // e.g. a view-controlled layer is brittle.
        let layer = CALayer::new();
        root_layer.addSublayer(&layer);

        // Set the anchor point and geometry. Softbuffer's uses a coordinate system with the origin
        // in the top-left corner.
        //
        // NOTE: This doesn't really matter unless we start modifying the `position` of our layer
        // ourselves, but it's nice to have in place.
        layer.setAnchorPoint(CGPoint::new(0.0, 0.0));
        layer.setGeometryFlipped(true);

        // Do not use auto-resizing mask.
        //
        // This is done to work around a bug in macOS 14 and above, where views using auto layout
        // may end up setting fractional values as the bounds, and that in turn doesn't propagate
        // properly through the auto-resizing mask and with contents gravity.
        //
        // Instead, we keep the bounds of the layer in sync with the root layer using an observer,
        // see below.
        //
        // layer.setAutoresizingMask(kCALayerHeightSizable | kCALayerWidthSizable);

        let observer = Observer::new(&layer);
        // Observe changes to the root layer's bounds and scale factor, and apply them to our layer.
        //
        // The previous implementation updated the scale factor inside `resize`, but this works
        // poorly with transactions, and is generally inefficient. Instead, we update the scale
        // factor only when needed because the super layer's scale factor changed.
        //
        // Note that inherent in this is an explicit design decision: We control the `bounds` and
        // `contentsScale` of the layer directly, and instead let the `resize` call that the user
        // controls only be the size of the underlying buffer.
        //
        // SAFETY: Observer deregistered in `Drop` before the observer object is deallocated.
        unsafe {
            root_layer.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("contentsScale"),
                NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Initial,
                ptr::null_mut(),
            );
            root_layer.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("bounds"),
                NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Initial,
                ptr::null_mut(),
            );
        }

        // Set the content so that it is placed in the top-left corner if it does not have the same
        // size as the surface itself.
        //
        // TODO(madsmtm): Consider changing this to `kCAGravityResize` to stretch the content if
        // resized to something that doesn't fit, see #177.
        layer.setContentsGravity(unsafe { kCAGravityTopLeft });

        // Initialize color space here, to reduce work later on.
        let color_space = CGColorSpace::new_device_rgb().unwrap();

        // Grab initial width and height from the layer (whose properties have just been initialized
        // by the observer using `NSKeyValueObservingOptionInitial`).
        let size = layer.bounds().size;
        let scale_factor = layer.contentsScale();
        let width = (size.width * scale_factor) as usize;
        let height = (size.height * scale_factor) as usize;

        Ok(Self {
            layer: SendCALayer(layer),
            root_layer: SendCALayer(root_layer),
            observer,
            color_space,
            width,
            height,
            pool: BufferPool::new(),
            _display: PhantomData,
            window_handle: window_src,
        })
    }

    #[inline]
    fn window(&self) -> &W {
        &self.window_handle
    }

    fn resize(&mut self, width: NonZeroU32, height: NonZeroU32) -> Result<(), SoftBufferError> {
        self.width = width.get() as usize;
        self.height = height.get() as usize;
        Ok(())
    }

    fn buffer_mut(&mut self) -> Result<BufferImpl<'_, D, W>, SoftBufferError> {
        let (buffer, age) = self.pool.take(self.width * self.height);
        Ok(BufferImpl {
            buffer,
            age,
            taken_at: age_tracing_enabled().then(std::time::Instant::now),
            imp: self,
        })
    }
}

#[derive(Debug)]
pub struct BufferImpl<'a, D, W> {
    imp: &'a mut CGImpl<D, W>,
    buffer: util::PixelBuffer,
    /// How many presents ago this buffer's pixels were last on screen.
    age: u8,
    /// When `buffer_mut` handed this over, so `present` can report how long
    /// the caller spent drawing. Only set while tracing.
    taken_at: Option<std::time::Instant>,
}

impl<D: HasDisplayHandle, W: HasWindowHandle> BufferInterface for BufferImpl<'_, D, W> {
    fn width(&self) -> NonZeroU32 {
        NonZeroU32::new(self.imp.width as u32).unwrap()
    }

    fn height(&self) -> NonZeroU32 {
        NonZeroU32::new(self.imp.height as u32).unwrap()
    }

    #[inline]
    fn pixels(&self) -> &[u32] {
        &self.buffer
    }

    #[inline]
    fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.buffer
    }

    fn age(&self) -> u8 {
        self.age
    }

    fn present(self) -> Result<(), SoftBufferError> {
        unsafe extern "C-unwind" fn release(info: *mut c_void, data: NonNull<c_void>, size: usize) {
            let data = data.cast::<u32>();
            let slice = slice_from_raw_parts_mut(data.as_ptr(), size / size_of::<u32>());
            // SAFETY: This is the same slice that we passed to `Box::into_raw` below.
            let pixels = unsafe { Box::from_raw(slice) };

            // SAFETY: `present` passed an `Arc::into_raw` pointer as the info
            // pointer, and Core Graphics calls this exactly once per data
            // provider, so this reclaims that one reference and no other.
            let pool = unsafe { Arc::from_raw(info.cast::<BufferPool>()) };

            // Hand the allocation back instead of freeing it, so the next
            // frame can draw over it rather than faulting in a fresh mapping.
            // Core Graphics may call this from any thread; the pool locks.
            pool.reclaim(pixels);
        }

        if let Some(taken_at) = self.taken_at {
            trace_draw(taken_at.elapsed());
        }

        let started = age_tracing_enabled().then(std::time::Instant::now);
        let pool = Arc::clone(&self.imp.pool);

        let data_provider = {
            let len = self.buffer.len() * size_of::<u32>();
            let buffer: *mut [u32] = Box::into_raw(self.buffer.0.into_boxed_slice());
            // Convert slice pointer to thin pointer.
            let data_ptr = buffer.cast::<c_void>();

            pool.mark_presented(data_ptr as usize);

            // SAFETY: The data pointer and length are valid. The info pointer
            // is a leaked `Arc<BufferPool>` reference that `release` reclaims.
            unsafe {
                CGDataProvider::with_data(
                    Arc::into_raw(Arc::clone(&pool)).cast_mut().cast::<c_void>(),
                    data_ptr,
                    len,
                    Some(release),
                )
                .unwrap()
            }
        };

        // `CGBitmapInfo` consists of a combination of `CGImageAlphaInfo`, `CGImageComponentInfo`
        // `CGImageByteOrderInfo` and `CGImagePixelFormatInfo` (see e.g. `CGBitmapInfoMake`).
        //
        // TODO: Use `CGBitmapInfo::new` once the next version of objc2-core-graphics is released.
        let bitmap_info = CGBitmapInfo(
            CGImageAlphaInfo::NoneSkipFirst.0
                | CGImageComponentInfo::Integer.0
                | CGImageByteOrderInfo::Order32Little.0
                | CGImagePixelFormatInfo::Packed.0,
        );

        let image = unsafe {
            CGImage::new(
                self.imp.width,
                self.imp.height,
                8,
                32,
                self.imp.width * 4,
                Some(&self.imp.color_space),
                bitmap_info,
                Some(&data_provider),
                ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .unwrap();

        // The CALayer has a default action associated with a change in the layer contents, causing
        // a quarter second fade transition to happen every time a new buffer is applied. This can
        // be avoided by wrapping the operation in a transaction and disabling all actions.
        CATransaction::begin();
        CATransaction::setDisableActions(true);

        // SAFETY: The contents is `CGImage`, which is a valid class for `contents`.
        unsafe { self.imp.layer.setContents(Some(image.as_ref())) };

        CATransaction::commit();

        if let Some(started) = started {
            trace_present(started.elapsed());
        }

        Ok(())
    }

    fn present_with_damage(self, _damage: &[Rect]) -> Result<(), SoftBufferError> {
        self.present()
    }
}

#[derive(Debug)]
struct SendCALayer(Retained<CALayer>);

// SAFETY: CALayer is dubiously thread safe, like most things in Core Animation.
// But since we make sure to do our changes within a CATransaction, it is
// _probably_ fine for us to use CALayer from different threads.
//
// See also:
// https://developer.apple.com/documentation/quartzcore/catransaction/1448267-lock?language=objc
// https://stackoverflow.com/questions/76250226/how-to-render-content-of-calayer-on-a-background-thread
unsafe impl Send for SendCALayer {}
// SAFETY: Same as above.
unsafe impl Sync for SendCALayer {}

impl Deref for SendCALayer {
    type Target = CALayer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands a buffer to the pool the way `present` does, and returns it in
    /// the state Core Graphics holds it in until the layer swaps contents.
    fn present(pool: &BufferPool, pixels: util::PixelBuffer) -> Box<[u32]> {
        let held = pixels.0.into_boxed_slice();
        pool.mark_presented(held.as_ptr() as usize);
        held
    }

    #[test]
    fn a_cold_pool_allocates_and_says_the_pixels_are_undefined() {
        let pool = BufferPool::new();

        let (pixels, age) = pool.take(16);

        assert_eq!(age, 0);
        assert_eq!(pixels.len(), 16);
        assert!(pixels.iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn a_returned_buffer_comes_back_with_its_age() {
        let pool = BufferPool::new();

        let (pixels, _) = pool.take(16);
        let held = present(&pool, pixels);
        pool.reclaim(held);

        let (_, age) = pool.take(16);

        assert_eq!(age, 1);
    }

    #[test]
    fn the_rotation_settles_at_two_buffers() {
        let pool = BufferPool::new();
        let mut on_screen = None;
        let mut ages = Vec::new();

        for _ in 0..6 {
            let (pixels, age) = pool.take(16);
            ages.push(age);

            // Core Graphics releases the previous frame when the layer takes
            // the new one, so a buffer is never free the frame after it went
            // out.
            if let Some(previous) = on_screen.replace(present(&pool, pixels)) {
                pool.reclaim(previous);
            }
        }

        assert_eq!(ages, vec![0, 0, 2, 2, 2, 2]);
    }

    #[test]
    fn a_reused_buffer_keeps_the_pixels_it_was_showing() {
        let pool = BufferPool::new();

        let (mut pixels, _) = pool.take(4);
        pixels.0.copy_from_slice(&[1, 2, 3, 4]);
        let held = present(&pool, pixels);
        pool.reclaim(held);

        let (pixels, age) = pool.take(4);

        // The pixels are the whole point: an age above zero promises the
        // caller they survived, and it will only repaint what changed.
        assert_ne!(age, 0);
        assert_eq!(pixels.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    fn a_resize_retires_the_buffers_allocated_at_the_old_size() {
        let pool = BufferPool::new();

        let (pixels, _) = pool.take(16);
        let held = present(&pool, pixels);
        pool.reclaim(held);

        let (pixels, age) = pool.take(64);

        assert_eq!(age, 0);
        assert_eq!(pixels.len(), 64);
    }

    #[test]
    fn the_pool_stops_growing_at_its_cap() {
        let pool = BufferPool::new();

        // More buffers in flight at once than a rotation ever needs.
        let held: Vec<_> = (0..MAX_POOLED_BUFFERS + 2)
            .map(|_| present(&pool, pool.take(4).0))
            .collect();
        for buffer in held {
            pool.reclaim(buffer);
        }

        assert_eq!(pool.lock().available.len(), MAX_POOLED_BUFFERS);
    }
}
