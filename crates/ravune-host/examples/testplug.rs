//! A tiny CLAP instrument for ravune-host's tests: a sine per held note at "Level"
//! (parameter 1, 0..1), which is also its saved state.

use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts, PluginAudioPortsImpl};
use clack_extensions::note_ports::{NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts, PluginNotePortsImpl};
use clack_extensions::params::{ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams, PluginMainThreadParams, PluginParams};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};

const LEVEL: u32 = 1;

pub struct TestTone;

pub struct Shared {
    level: AtomicU64,
}

impl Shared {
    fn level(&self) -> f64 {
        f64::from_bits(self.level.load(Relaxed))
    }

    fn set_level(&self, v: f64) {
        self.level.store(v.clamp(0.0, 1.0).to_bits(), Relaxed);
    }

    fn apply(&self, events: &InputEvents) {
        for e in events {
            if let Some(CoreEventSpace::ParamValue(p)) = e.as_core_event()
                && p.param_id().map(|i| i.get()) == Some(LEVEL)
            {
                self.set_level(p.value());
            }
        }
    }
}

impl PluginShared<'_> for Shared {}

pub struct MainThread<'a> {
    shared: &'a Shared,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

impl Plugin for TestTone {
    type AudioProcessor<'a> = Tone<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder.register::<PluginAudioPorts>().register::<PluginNotePorts>().register::<PluginParams>().register::<PluginState>();
    }
}

impl DefaultPluginFactory for TestTone {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("com.ravune.test-tone", "Test Tone").with_vendor("Ravune").with_version("1.0").with_features([c"instrument", c"synthesizer"])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        Ok(Shared { level: AtomicU64::new(0.5f64.to_bits()) })
    }

    fn new_main_thread<'a>(_host: HostMainThreadHandle<'a>, shared: &'a Shared) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread { shared })
    }
}

pub struct Tone<'a> {
    shared: &'a Shared,
    rate: f64,
    /// (key, phase) of the held notes.
    notes: Vec<(u16, f64)>,
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Tone<'a> {
    fn activate(_host: HostAudioProcessorHandle<'a>, _main: &MainThread<'a>, shared: &'a Shared, config: PluginAudioConfiguration) -> Result<Self, PluginError> {
        Ok(Self { shared, rate: config.sample_rate, notes: Vec::with_capacity(32) })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, events: Events) -> Result<ProcessStatus, PluginError> {
        for e in events.input {
            match e.as_core_event() {
                Some(CoreEventSpace::NoteOn(n)) => self.notes.push((n.as_raw().key as u16, 0.0)),
                Some(CoreEventSpace::NoteOff(n)) => self.notes.retain(|x| x.0 != n.as_raw().key as u16),
                _ => {}
            }
        }
        self.shared.apply(events.input);
        let level = self.shared.level() as f32;
        let mut frames = 0;
        for mut port in &mut audio {
            let Some(channels) = port.channels()?.into_f32() else { continue };
            for pair in channels {
                let buf = match pair {
                    ChannelPair::OutputOnly(b) | ChannelPair::InPlace(b) => b,
                    ChannelPair::InputOutput(_, b) => b,
                    ChannelPair::InputOnly(_) => continue,
                };
                for (i, s) in buf.iter_mut().enumerate() {
                    *s = self
                        .notes
                        .iter()
                        .map(|&(key, phase)| {
                            let hz = 440.0 * ((key as f64 - 69.0) / 12.0).exp2();
                            ((phase + i as f64 * hz / self.rate) * std::f64::consts::TAU).sin() as f32
                        })
                        .sum::<f32>()
                        * level;
                }
                frames = buf.len();
            }
        }
        for (key, phase) in &mut self.notes {
            *phase = (*phase + frames as f64 * 440.0 * ((*key as f64 - 69.0) / 12.0).exp2() / self.rate).fract();
        }
        Ok(ProcessStatus::Continue)
    }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !is_input {
            writer.set(&AudioPortInfo { id: ClapId::new(0), name: b"Out", channel_count: 2, flags: AudioPortFlags::IS_MAIN, port_type: Some(AudioPortType::STEREO), in_place_pair: None });
        }
    }
}

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 1 } else { 0 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo { id: ClapId::new(0), name: b"Notes", supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI, preferred_dialect: Some(NoteDialect::Clap) });
        }
    }
}

impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.level().to_le_bytes())?;
        Ok(())
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = [0u8; 8];
        input.read_exact(&mut bytes)?;
        self.shared.set_level(f64::from_le_bytes(bytes));
        Ok(())
    }
}

impl PluginMainThreadParams for MainThread<'_> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        if index == 0 {
            info.set(&ParamInfo { id: ClapId::new(LEVEL), flags: ParamInfoFlags::IS_AUTOMATABLE, cookie: Default::default(), name: b"Level", module: b"", min_value: 0.0, max_value: 1.0, default_value: 0.5 });
        }
    }

    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        (param_id.get() == LEVEL).then(|| self.shared.level())
    }

    fn value_to_text(&self, _param_id: ClapId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
        write!(writer, "{value:.2}")
    }

    fn text_to_value(&self, _param_id: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}

impl PluginAudioProcessorParams for Tone<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}

clack_export_entry!(SinglePluginEntry<TestTone>);
