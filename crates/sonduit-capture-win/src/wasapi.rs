//! WASAPI loopback capture.
//!
//! # The silent render stream
//!
//! Endpoint loopback is documented as event-driven since Windows 10 1703, but
//! the event is driven by *render activity*: with nothing playing, the handle
//! is never signalled, `GetBuffer` returns nothing, and a naive reader blocks
//! forever while the stream still reports itself as running. NAudio documents
//! the same behaviour and PortAudio has an open issue for it.
//!
//! The fix, and the one Microsoft itself documented as the pre-1703 workaround,
//! is to keep a silent render stream open on the same endpoint. It does double
//! duty here: it keeps the engine clocking so loopback keeps producing, and
//! because the whole engine adopts the smallest period any client asks for, it
//! also pulls the period below the 10 ms default. That matters because
//! `IAudioClient3::InitializeSharedAudioStream` rejects the loopback flag, so
//! the period cannot be requested from the capture side at all.
//!
//! See `docs/research/wasapi-vs-virtual-driver.md` and ADR-002.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sonduit_core::format::{BitDepth, Format};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::{CaptureError, Endpoint};

/// `PKEY_Device_FriendlyName`, which the SDK exposes only as a macro.
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// 100-nanosecond units in one millisecond, the unit WASAPI durations use.
const HNS_PER_MS: i64 = 10_000;

/// How long the capture loop waits for the engine before deciding it is stuck.
///
/// The silent render stream should keep the event firing, but a timeout means
/// a stall is reported rather than hanging the thread forever.
const WAIT_TIMEOUT_MS: u32 = 2_000;

/// RAII wrapper so COM is uninitialised on every exit path.
struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self, CaptureError> {
        // SAFETY: called once per thread before any other COM call, and
        // balanced by CoUninitialize in Drop.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_err() {
            return Err(CaptureError::Platform(format!(
                "CoInitializeEx failed: {result:?}"
            )));
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: balances the CoInitializeEx in new().
        unsafe { CoUninitialize() };
    }
}

/// A Win32 event handle that closes itself.
struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self, CaptureError> {
        // SAFETY: null arguments request an unnamed auto-reset event.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|error| CaptureError::Platform(format!("CreateEventW failed: {error}")))?;
        Ok(Self(handle))
    }

    /// Wake anything waiting, used to break the loop out of its wait on stop.
    fn signal(&self) {
        // SAFETY: the handle is valid for the lifetime of self.
        let _ = unsafe { SetEvent(self.0) };
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateEventW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: a Win32 event handle is a kernel object; signalling it from another
// thread is exactly what it exists for.
unsafe impl Send for EventHandle {}
// SAFETY: as above; SetEvent is documented as thread-safe.
unsafe impl Sync for EventHandle {}

fn device_enumerator() -> Result<IMMDeviceEnumerator, CaptureError> {
    // SAFETY: standard COM activation of a documented class.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|error| CaptureError::Platform(format!("MMDeviceEnumerator failed: {error}")))
}

fn device_id(device: &IMMDevice) -> Result<String, CaptureError> {
    // SAFETY: GetId returns a COM-allocated wide string we must free.
    let raw = unsafe { device.GetId() }
        .map_err(|error| CaptureError::Platform(format!("GetId failed: {error}")))?;
    // SAFETY: the pointer is a valid null-terminated wide string.
    let value = unsafe { raw.to_string() }.unwrap_or_default();
    // SAFETY: freeing the buffer GetId allocated.
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Ok(value)
}

fn device_name(device: &IMMDevice) -> String {
    // SAFETY: opening the property store read-only.
    let Ok(store) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
        return "Unknown device".to_string();
    };
    // SAFETY: reading a documented property key.
    let Ok(mut value) = (unsafe { store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) }) else {
        return "Unknown device".to_string();
    };
    // SAFETY: the variant holds a wide string for this key.
    let name = unsafe { value.Anonymous.Anonymous.Anonymous.pwszVal.to_string() }
        .unwrap_or_else(|_| "Unknown device".to_string());
    // SAFETY: releasing the variant we were handed.
    let _ = unsafe { PropVariantClear(&mut value) };
    name
}

/// The identifier of the endpoint Windows currently hands new streams.
///
/// `None` when the machine has no default render device, which is a machine
/// with nothing to capture rather than an error worth failing enumeration on.
fn default_render_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    // SAFETY: eRender with the console role is the device the user hears.
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .ok()
        .and_then(|device| device_id(&device).ok())
}

/// Find the endpoint to capture, falling back to the console-role default.
///
/// A chosen device that is unplugged, disabled or simply gone falls back
/// rather than failing the session: silence with an explanation is worse than
/// audio from the speakers the user can actually hear. The caller reports the
/// endpoint that was opened, so the panel says which one that was.
fn resolve_device(
    enumerator: &IMMDeviceEnumerator,
    requested: Option<&str>,
) -> Result<IMMDevice, CaptureError> {
    if let Some(id) = requested.filter(|id| !id.is_empty()) {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: GetDevice takes a null-terminated wide string, and `wide`
        // stays alive for the duration of the call.
        if let Ok(device) = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) } {
            // GetDevice still returns a device that has been unplugged or
            // disabled. Activating one fails later and with a worse message,
            // so the state is checked here instead.
            // SAFETY: reading the state of a device we hold.
            let active =
                unsafe { device.GetState() }.is_ok_and(|state| state == DEVICE_STATE_ACTIVE);
            if active {
                return Ok(device);
            }
        }
    }

    // SAFETY: the console role is the endpoint the user actually hears.
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|_| CaptureError::NoEndpoint)
}

/// Describe a device we hold, marking it if it is the current default.
fn describe(device: &IMMDevice, default_id: Option<&str>) -> Endpoint {
    let id = device_id(device).unwrap_or_default();
    Endpoint {
        is_default: default_id == Some(id.as_str()),
        name: device_name(device),
        id,
    }
}

/// List active render endpoints, marking the default one.
pub fn enumerate() -> Result<Vec<Endpoint>, CaptureError> {
    let _com = ComGuard::new()?;
    let enumerator = device_enumerator()?;

    let default_id = default_render_id(&enumerator);

    // SAFETY: enumerating active render endpoints.
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|error| CaptureError::Platform(format!("EnumAudioEndpoints failed: {error}")))?;

    // SAFETY: the collection reports its own length.
    let count = unsafe { collection.GetCount() }
        .map_err(|error| CaptureError::Platform(format!("GetCount failed: {error}")))?;

    let mut endpoints = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: index is bounded by GetCount.
        let Ok(device) = (unsafe { collection.Item(index) }) else {
            continue;
        };
        endpoints.push(describe(&device, default_id.as_deref()));
    }

    if endpoints.is_empty() {
        return Err(CaptureError::NoEndpoint);
    }
    Ok(endpoints)
}

/// Read the format an endpoint's engine is actually mixing at.
///
/// The mix format is authoritative: asking for anything else in shared mode is
/// refused, and a mismatch is the single largest latency mistake available
/// (see `docs/research/android-aaudio.md` for the same trap on the other side).
fn mix_format(client: &IAudioClient) -> Result<(*mut WAVEFORMATEX, Format), CaptureError> {
    // SAFETY: GetMixFormat allocates a format we own and must free.
    let raw = unsafe { client.GetMixFormat() }
        .map_err(|error| CaptureError::Platform(format!("GetMixFormat failed: {error}")))?;

    // SAFETY: the pointer is non-null on success.
    let wave = unsafe { &*raw };
    let channels = wave.nChannels as u8;
    let sample_rate = wave.nSamplesPerSec;
    let bits = wave.wBitsPerSample;

    // The engine mixes in 32-bit float. Sonduit's wire format carries integer
    // PCM, so the capture loop converts; the Format reported here is what will
    // go on the wire, not what WASAPI hands over.
    let bit_depth = BitDepth::S16;

    let channel_mask = if wave.cbSize as usize
        >= core::mem::size_of::<WAVEFORMATEXTENSIBLE>() - core::mem::size_of::<WAVEFORMATEX>()
    {
        // SAFETY: cbSize says the extensible fields are present.
        let extensible = unsafe { &*raw.cast::<WAVEFORMATEXTENSIBLE>() };
        extensible.dwChannelMask as u16
    } else {
        match channels {
            1 => 0x0004,
            _ => 0x0003,
        }
    };

    let format = Format {
        sample_rate,
        bit_depth,
        channels,
        channel_mask,
    };

    if format.validate().is_err() {
        // SAFETY: freeing the format before bailing out.
        unsafe { CoTaskMemFree(Some(raw.cast())) };
        return Err(CaptureError::Platform(format!(
            "engine mix format is unusable: {sample_rate} Hz, {channels} ch, {bits} bit"
        )));
    }

    Ok((raw, format))
}

/// A silent render client, kept alive so loopback keeps producing.
struct Keepalive {
    client: IAudioClient,
    render: IAudioRenderClient,
    buffer_frames: u32,
}

impl Keepalive {
    fn start(device: &IMMDevice, period_hns: i64) -> Result<Self, CaptureError> {
        // SAFETY: activating the audio client on a device we hold.
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| CaptureError::Platform(format!("keepalive Activate: {error}")))?;

        // SAFETY: GetMixFormat allocates; freed below.
        let format = unsafe { client.GetMixFormat() }
            .map_err(|error| CaptureError::Platform(format!("keepalive GetMixFormat: {error}")))?;

        // SAFETY: a plain shared-mode render stream at the requested period.
        let result =
            unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, period_hns, 0, format, None) };
        // SAFETY: the format is no longer needed once Initialize has copied it.
        unsafe { CoTaskMemFree(Some(format.cast())) };
        result.map_err(|error| CaptureError::Platform(format!("keepalive Initialize: {error}")))?;

        // SAFETY: the client is initialised.
        let buffer_frames = unsafe { client.GetBufferSize() }
            .map_err(|error| CaptureError::Platform(format!("keepalive GetBufferSize: {error}")))?;
        // SAFETY: requesting the render service on an initialised client.
        let render: IAudioRenderClient = unsafe { client.GetService() }
            .map_err(|error| CaptureError::Platform(format!("keepalive GetService: {error}")))?;

        // Prime the whole buffer with silence before starting, or the first
        // pass renders whatever happened to be in it.
        // SAFETY: buffer_frames came from GetBufferSize.
        if let Ok(data) = unsafe { render.GetBuffer(buffer_frames) } {
            // SAFETY: releasing exactly what was acquired, flagged silent so
            // the contents are ignored.
            let _ =
                unsafe { render.ReleaseBuffer(buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) };
            let _ = data;
        }

        // SAFETY: starting an initialised client.
        unsafe { client.Start() }
            .map_err(|error| CaptureError::Platform(format!("keepalive Start: {error}")))?;

        Ok(Self {
            client,
            render,
            buffer_frames,
        })
    }

    /// Top the buffer back up with silence.
    fn pump(&self) {
        // SAFETY: padding of an initialised client.
        let Ok(padding) = (unsafe { self.client.GetCurrentPadding() }) else {
            return;
        };
        let available = self.buffer_frames.saturating_sub(padding);
        if available == 0 {
            return;
        }
        // SAFETY: available is bounded by the buffer size.
        if unsafe { self.render.GetBuffer(available) }.is_ok() {
            // SAFETY: releasing exactly what was acquired.
            let _ = unsafe {
                self.render
                    .ReleaseBuffer(available, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)
            };
        }
    }
}

impl Drop for Keepalive {
    fn drop(&mut self) {
        // SAFETY: stopping a started client.
        let _ = unsafe { self.client.Stop() };
    }
}

/// A running loopback capture.
///
/// Lives entirely on the thread that created it: WASAPI interfaces are
/// apartment-bound and the COM guard is per-thread.
pub struct LoopbackCapture {
    _com: ComGuard,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Arc<EventHandle>,
    format: Format,
    /// Channels the engine mixes, which drives the float-to-integer conversion.
    engine_channels: usize,
    /// The endpoint this client is actually tapping.
    endpoint: Endpoint,
    /// The endpoint the caller asked for, which may not be the one above.
    ///
    /// Kept so a reopen goes back to the chosen device once it returns,
    /// instead of settling permanently on whatever the fallback found.
    requested: Option<String>,
    _keepalive: Option<Keepalive>,
    stopping: Arc<AtomicBool>,
}

impl LoopbackCapture {
    /// Open loopback capture on a render endpoint.
    ///
    /// `endpoint_id` is an identifier from [`enumerate`]. `None`, or an id
    /// naming a device that is no longer active, opens the console-role
    /// default instead; [`LoopbackCapture::endpoint`] reports which endpoint
    /// that turned out to be.
    ///
    /// `period_ms` is the engine period to request through the keepalive
    /// stream. Smaller is lower latency and more likely to glitch.
    pub fn open(period_ms: u32, endpoint_id: Option<&str>) -> Result<Self, CaptureError> {
        let com = ComGuard::new()?;
        let enumerator = device_enumerator()?;

        let device = resolve_device(&enumerator, endpoint_id)?;
        let endpoint = describe(&device, default_render_id(&enumerator).as_deref());

        let period_hns = i64::from(period_ms.max(1)) * HNS_PER_MS;

        // The keepalive has to exist before capture starts, so the engine is
        // already clocking when the loopback client attaches.
        let keepalive = Keepalive::start(&device, period_hns).ok();

        // SAFETY: activating the audio client on a device we hold.
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| CaptureError::Platform(format!("Activate failed: {error}")))?;

        let (raw_format, format) = mix_format(&client)?;

        // SAFETY: the format pointer is valid until freed below.
        let engine_channels = unsafe { (*raw_format).nChannels } as usize;

        // SAFETY: loopback plus event callback on a shared-mode stream. The
        // buffer is two periods so a late wake-up does not immediately drop
        // audio.
        let result = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                period_hns * 2,
                0,
                raw_format,
                None,
            )
        };
        // SAFETY: Initialize has copied what it needs.
        unsafe { CoTaskMemFree(Some(raw_format.cast())) };
        result.map_err(|error| CaptureError::Platform(format!("Initialize failed: {error}")))?;

        let event = Arc::new(EventHandle::new()?);
        // SAFETY: the handle outlives the client, which is stopped in Drop.
        unsafe { client.SetEventHandle(event.0) }
            .map_err(|error| CaptureError::Platform(format!("SetEventHandle failed: {error}")))?;

        // SAFETY: requesting the capture service on an initialised client.
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|error| CaptureError::Platform(format!("GetService failed: {error}")))?;

        // SAFETY: starting an initialised client.
        unsafe { client.Start() }
            .map_err(|error| CaptureError::Platform(format!("Start failed: {error}")))?;

        Ok(Self {
            _com: com,
            client,
            capture,
            event,
            format,
            engine_channels,
            endpoint,
            requested: endpoint_id.map(ToString::to_string),
            _keepalive: keepalive,
            stopping: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Format the captured audio will be delivered in.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }

    /// The endpoint this client is tapping, which is what the user is told.
    ///
    /// Not the same as the one asked for when that device was unavailable, and
    /// the difference is the whole point of reporting it rather than echoing
    /// the request back.
    #[must_use]
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The endpoint the caller asked for, for a later reopen to ask again.
    #[must_use]
    pub fn requested_endpoint(&self) -> Option<&str> {
        self.requested.as_deref()
    }

    /// A handle that stops the capture loop from another thread.
    #[must_use]
    pub fn stopper(&self) -> CaptureStopper {
        CaptureStopper {
            stopping: Arc::clone(&self.stopping),
            event: Arc::clone(&self.event),
        }
    }

    /// Wait for the next block and append it to `out` as interleaved 16-bit PCM.
    ///
    /// Returns the number of frames appended. Zero means the wait timed out or
    /// the capture was stopped, both of which the caller should treat as "no
    /// audio this round" rather than as an error.
    pub fn read(&mut self, out: &mut Vec<u8>) -> Result<usize, CaptureError> {
        if self.stopping.load(Ordering::Relaxed) {
            return Ok(0);
        }

        // SAFETY: waiting on the event the client signals.
        let wait = unsafe { WaitForSingleObject(self.event.0, WAIT_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            // The keepalive should prevent this, so a timeout is worth
            // surfacing as silence rather than as data.
            return Ok(0);
        }
        if self.stopping.load(Ordering::Relaxed) {
            return Ok(0);
        }

        if let Some(keepalive) = &self._keepalive {
            keepalive.pump();
        }

        let mut frames_written = 0;

        loop {
            // SAFETY: asking how much is ready before acquiring it.
            let ready = unsafe { self.capture.GetNextPacketSize() }.map_err(|error| {
                CaptureError::Platform(format!("GetNextPacketSize failed: {error}"))
            })?;
            if ready == 0 {
                break;
            }

            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;

            // SAFETY: GetBuffer hands back a pointer valid until ReleaseBuffer.
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            }
            .map_err(|error| CaptureError::Platform(format!("GetBuffer failed: {error}")))?;

            if frames > 0 {
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    // The engine says this block is silence and the buffer
                    // contents are undefined, so write real zeroes.
                    out.extend(std::iter::repeat_n(
                        0_u8,
                        frames as usize * self.format.bytes_per_frame(),
                    ));
                } else {
                    // SAFETY: the engine mixes 32-bit float, `frames` frames of
                    // `engine_channels` samples each, in the buffer just
                    // acquired.
                    let samples = unsafe {
                        std::slice::from_raw_parts(
                            data.cast::<f32>(),
                            frames as usize * self.engine_channels,
                        )
                    };
                    append_as_i16(samples, out);
                }
                frames_written += frames as usize;
            }

            // SAFETY: releasing exactly the frames GetBuffer reported.
            unsafe { self.capture.ReleaseBuffer(frames) }.map_err(|error| {
                CaptureError::Platform(format!("ReleaseBuffer failed: {error}"))
            })?;
        }

        Ok(frames_written)
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        // SAFETY: stopping a started client.
        let _ = unsafe { self.client.Stop() };
    }
}

/// Stops a [`LoopbackCapture`] from another thread.
#[derive(Clone)]
pub struct CaptureStopper {
    stopping: Arc<AtomicBool>,
    event: Arc<EventHandle>,
}

impl CaptureStopper {
    /// Ask the capture loop to finish and wake it if it is waiting.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.event.signal();
    }
}

/// Convert the engine's float samples to the interleaved 16-bit PCM the wire
/// format carries.
///
/// Values outside -1.0..=1.0 are clamped rather than allowed to wrap, because a
/// wrapped sample is full-scale noise.
fn append_as_i16(samples: &[f32], out: &mut Vec<u8>) {
    out.reserve(samples.len() * 2);
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_samples_convert_to_little_endian_i16() {
        let mut out = Vec::new();
        append_as_i16(&[0.0, 1.0, -1.0], &mut out);

        assert_eq!(out.len(), 6);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), 0);
        assert_eq!(i16::from_le_bytes([out[2], out[3]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([out[4], out[5]]), -i16::MAX);
    }

    #[test]
    fn samples_beyond_full_scale_clamp_instead_of_wrapping() {
        // A wrapped sample is full-scale noise, which is far worse than a
        // clipped one. The engine can hand back values above 1.0 when several
        // loud streams are mixed.
        let mut out = Vec::new();
        append_as_i16(&[4.0, -4.0, 1.5, -1.5], &mut out);

        let values: Vec<i16> = out
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();

        assert_eq!(values, vec![i16::MAX, -i16::MAX, i16::MAX, -i16::MAX]);
    }

    #[test]
    fn a_non_finite_sample_does_not_panic_or_wrap() {
        let mut out = Vec::new();
        append_as_i16(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY], &mut out);

        let values: Vec<i16> = out
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();

        // clamp propagates NaN, and Rust's float-to-integer cast saturates,
        // sending NaN to zero rather than to an unspecified bit pattern. Both
        // infinities clamp to full scale. Silence and clipping are survivable;
        // an undefined sample is not.
        assert_eq!(values, vec![0, i16::MAX, -i16::MAX]);
    }

    #[test]
    fn an_empty_block_produces_no_bytes() {
        let mut out = Vec::new();
        append_as_i16(&[], &mut out);
        assert!(out.is_empty());
    }
}
