use crate::backend::{Backend, Renderer};
use cpal::{
	Device, Stream, StreamConfig,
	traits::{DeviceTrait, HostTrait, StreamTrait},
};
use send_wrapper::SendWrapper;

use super::{CpalBackendSettings, Error};

enum State {
	Empty,
	Uninitialized {
		device: Device,
		config: StreamConfig,
	},
	Initialized {
		_stream: Stream,
	},
}

/// A backend that uses [cpal](https://crates.io/crates/cpal) to
/// connect a [`Renderer`] to the operating system's audio driver.
pub struct CpalBackend {
	state: SendWrapper<State>,
}

impl Backend for CpalBackend {
	type Settings = CpalBackendSettings;

	type Error = Error;

	fn setup(
		settings: Self::Settings,
		_internal_buffer_size: usize,
	) -> Result<(Self, u32), Self::Error> {
		let host = cpal::default_host();
		let device = if let Some(device) = settings.device {
			device
		} else {
			host.default_output_device()
				.ok_or(Error::NoDefaultOutputDevice)?
		};
		let mut config = if let Some(config) = settings.config {
			config
		} else {
			device.default_output_config()?.config()
		};
		// SHURLEY PATCH: cpal's webaudio backend defaults to 2048 frames
		// (~46ms) and schedules refills from the BROWSER MAIN THREAD -- the
		// same thread the whole single-threaded wasm app runs on, so any
		// long frame (texture upload, pdfium render) starves the buffer and
		// crackles. 4096 (~85ms at 48k) buys headroom.
		//
		// DO NOT RAISE THIS without also raising media's END_EPSILON. Playback
		// position only updates per chunk and this host schedules a chunk ahead,
		// so the last sample before a clip stops can sit up to TWO chunks short
		// of its declared duration. media advances a playlist on
		// `Stopped && current_time >= end - END_EPSILON` (0.25s), so 8192
		// (170-341ms short) silently stopped SEDA advancing at every clip
		// change. 4096 stays inside the epsilon. Word-highlight sync reads the
		// same position, so it pays for depth here too.
		if matches!(config.buffer_size, cpal::BufferSize::Default) {
			config.buffer_size = cpal::BufferSize::Fixed(4096);
		}
		let sample_rate = config.sample_rate;
		Ok((
			Self {
				state: SendWrapper::new(State::Uninitialized { device, config }),
			},
			sample_rate,
		))
	}

	fn start(&mut self, mut renderer: Renderer) -> Result<(), Self::Error> {
		if let State::Uninitialized { device, config } =
			std::mem::replace(&mut *self.state, State::Empty)
		{
			let channels = config.channels;
			let stream = device.build_output_stream(
				config,
				move |data: &mut [f32], _| {
					renderer.on_start_processing();
					renderer.process(data, channels);
				},
				move |_| {},
				None,
			)?;
			stream.play()?;
			self.state = SendWrapper::new(State::Initialized { _stream: stream });
		} else {
			panic!("Cannot initialize the backend multiple times")
		}
		Ok(())
	}
}
