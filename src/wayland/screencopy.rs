//! Code adapted from [cosmic-app-list]
//!
//! [cosmic-app-list]: https://github.com/pop-os/cosmic-applets/blob/master/cosmic-app-list/src/wayland_handler.rs#L274

use std::{
  os::fd::AsFd,
  sync::{Arc, Condvar, Mutex, MutexGuard},
  time::Duration,
};

use cosmic::cctk::{
  self,
  screencopy::{
    CaptureFrame,
    CaptureOptions,
    CaptureSession,
    CaptureSource,
    Capturer,
    FailureReason,
    Formats,
    Frame,
    ScreencopyFrameData,
    ScreencopyFrameDataExt,
    ScreencopyHandler,
    ScreencopySessionData,
    ScreencopySessionDataExt,
    ScreencopyState,
  },
  sctk::{
    self,
    shm::{Shm, ShmHandler},
  },
  toplevel_info::ToplevelInfo,
  wayland_client::{
    Connection,
    Dispatch,
    QueueHandle,
    WEnum,
    protocol::{
      wl_buffer::WlBuffer,
      wl_shm::{self, WlShm},
      wl_shm_pool::WlShmPool,
    },
  },
};

use crate::wayland::{WaylandEvent, WaylandState};

cctk::delegate_screencopy!(WaylandState);
sctk::delegate_shm!(WaylandState);

/// Like [`WaylandState`], this is the shared state of the capture thread.
struct CaptureState {
  queue_handle: QueueHandle<WaylandState>,
  connection: Connection,
  wl_shm: WlShm,
  capturer: Capturer,
}

/// An image in a shared memory pool
pub(super) struct ShmImage<Fd: AsFd> {
  fd: Fd,
  pub(super) width: u32,
  pub(super) height: u32,
}

impl<Fd: AsFd> ShmImage<Fd> {
  /// Gets the image by memmapping it from the anonymous file
  pub(super) fn image(&self) -> anyhow::Result<image::RgbaImage> {
    let mmap = unsafe { memmap2::Mmap::map(&self.fd.as_fd())? };
    image::RgbaImage::from_raw(self.width, self.height, mmap.to_vec())
      .ok_or(anyhow::anyhow!("ShmImage calculated its size incorrectly"))
  }
}

#[derive(Default)]
struct Session {
  /// Buffer formattting info
  formats: Option<Formats>,

  /// Wayland's capture response
  res: Option<Result<(), WEnum<FailureReason>>>,
}

/// Guard around [`Session`]
#[derive(Default)]
struct SessionHandle {
  cond: Condvar,
  inner: Mutex<Session>,
}

#[derive(Default)]
struct SessionData {
  session: Arc<SessionHandle>,
  data: ScreencopySessionData,
}

impl ScreencopySessionDataExt for SessionData {
  fn screencopy_session_data(&self) -> &ScreencopySessionData { &self.data }
}

struct FrameData {
  session: CaptureSession,
  data: ScreencopyFrameData,
}

impl ScreencopyFrameDataExt for FrameData {
  fn screencopy_frame_data(&self) -> &ScreencopyFrameData { &self.data }
}

impl SessionHandle {
  pub(super) fn from_capture_session(capture_session: &CaptureSession) -> Option<&Self> {
    Some(&capture_session.data::<SessionData>()?.session)
  }

  /// Apply `f` to the [`Session`] and notify all waiters
  fn update<F: FnOnce(&mut Session)>(&self, f: F) {
    f(&mut self.inner.lock().unwrap());
    self.cond.notify_all();
  }

  /// Wait on a condition for `timeout`
  fn wait_while<F: FnMut(&Session) -> bool>(
    &self,
    timeout: Duration,
    mut cond: F,
  ) -> MutexGuard<'_, Session> {
    self
      .cond
      .wait_timeout_while(self.inner.lock().unwrap(), timeout, |data| cond(data))
      .unwrap()
      .0
  }
}

impl ScreencopyHandler for WaylandState {
  fn screencopy_state(&mut self) -> &mut ScreencopyState { &mut self.screencopy_state }

  fn init_done(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    session: &CaptureSession,
    formats: &Formats,
  ) {
    // notify waiting threads that the formats are initialized
    SessionHandle::from_capture_session(session)
      .unwrap()
      .update(|data| {
        data.formats = Some(formats.clone());
      });
  }

  fn ready(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    screencopy_frame: &CaptureFrame,
    _: Frame,
  ) {
    // notify waiting threads that the image was captured
    let session = &screencopy_frame.data::<FrameData>().unwrap().session;
    SessionHandle::from_capture_session(session)
      .unwrap()
      .update(|data| {
        data.res = Some(Ok(()));
      });
  }

  fn failed(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    screencopy_frame: &CaptureFrame,
    reason: WEnum<FailureReason>,
  ) {
    // notify waiting threads that the image failed to capture
    let session = &screencopy_frame.data::<FrameData>().unwrap().session;
    SessionHandle::from_capture_session(session)
      .unwrap()
      .update(|data| {
        data.res = Some(Err(reason));
      });
  }

  fn stopped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &CaptureSession) {}
}

impl ShmHandler for WaylandState {
  fn shm_state(&mut self) -> &mut Shm { &mut self.shm_state }
}

impl Dispatch<WlShmPool, ()> for WaylandState {
  fn event(
    _: &mut Self,
    _: &WlShmPool,
    _: <WlShmPool as cctk::wayland_client::Proxy>::Event,
    (): &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl Dispatch<WlBuffer, ()> for WaylandState {
  fn event(
    _: &mut Self,
    _: &WlBuffer,
    _: <WlBuffer as cctk::wayland_client::Proxy>::Event,
    (): &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

impl CaptureState {
  pub(super) fn capture_image<Fd: AsFd>(
    &self,
    toplevel: &ToplevelInfo,
    fd: Fd,
    size: Option<u32>,
  ) -> Option<ShmImage<Fd>> {
    let session = Arc::new(SessionHandle::default());

    let capture_session = self
      .capturer
      .create_session(
        &CaptureSource::Toplevel(toplevel.foreign_toplevel.clone()),
        CaptureOptions::empty(),
        &self.queue_handle,
        SessionData {
          session: session.clone(),
          data: ScreencopySessionData::default(),
        },
      )
      .ok()?;

    self.connection.flush().ok()?;

    let formats = session
      .wait_while(Duration::from_secs(10), |data| data.formats.is_none())
      .formats
      .take()?;

    let (w, h) = formats.buffer_size;
    if w == 0 || h == 0 {
      return None;
    }

    if !formats.shm_formats.contains(&wl_shm::Format::Abgr8888) {
      tracing::error!(
        "no compatible buffer format found. available formats: {:#?}",
        formats
      );
      return None;
    }

    // px area * sizeof(rgba int)
    let buf_size = w * h * 4;
    if let Some(size) = size {
      // if requested size != the exact required buffer size
      if size != buf_size {
        return None;
      }
    }
    else {
      let _ = rustix::fs::ftruncate(&fd, buf_size.into());
    }

    // create shared memory pool
    let pool = self
      .wl_shm
      .create_pool(fd.as_fd(), buf_size.cast_signed(), &self.queue_handle, ());

    // create image buffer in pool
    let buf = pool.create_buffer(
      0,
      w.cast_signed(),
      h.cast_signed(),
      w.cast_signed() * 4, // one row at a time
      wl_shm::Format::Abgr8888,
      &self.queue_handle,
      (),
    );

    // use buffer to get image
    capture_session.capture(&buf, &[], &self.queue_handle, FrameData {
      session: capture_session.clone(),
      data: ScreencopyFrameData::default(),
    });

    // wait for response
    let res = session
      .wait_while(Duration::from_secs(10), |data| data.res.is_none())
      .res
      .take()?;
    pool.destroy();
    buf.destroy();

    if res.is_ok() {
      Some(ShmImage {
        fd,
        width: w,
        height: h,
      })
    }
    else {
      None
    }
  }
}

impl WaylandState {
  /// Spawns a thread capture and send the image as a [`WaylandEvent`].
  #[allow(clippy::cast_possible_truncation)]
  #[allow(clippy::cast_precision_loss)]
  #[allow(clippy::cast_sign_loss)]
  pub(super) fn send_image(&self, toplevel: ToplevelInfo) {
    let tx = self.tx.clone();

    let state = CaptureState {
      queue_handle: self.queue_handle.clone(),
      connection: self.connection.clone(),
      wl_shm: self.shm_state.wl_shm().clone(),
      capturer: self.screencopy_state.capturer().clone(),
    };

    std::thread::spawn(move || -> anyhow::Result<()> {
      let name = "active-window-screencopy";
      let fd = rustix::fs::memfd_create(name, rustix::fs::MemfdFlags::CLOEXEC)?;

      let img = state.capture_image(&toplevel, fd, None);
      let Some(img) = img
      else {
        tracing::error!("failed to get screencopy image");
        return Ok(());
      };

      let img = img.image()?;

      // largest side
      let max = img.width().max(img.height());
      let ratio = max as f32 / 256.0;

      // if image (largest side) is larger than 256
      let img = if ratio > 1.0 {
        // scale down
        let w = (img.width() as f32 / ratio).round();
        let h = (img.height() as f32 / ratio).round();

        image::imageops::resize(
          &img,
          w as u32,
          h as u32,
          image::imageops::FilterType::Lanczos3,
        )
      }
      else {
        img
      };

      if let Err(why) = tx.unbounded_send(WaylandEvent::Image(toplevel, img)) {
        tracing::error!(%why, "failed to send image event");
      }

      Ok(())
    });
  }
}
