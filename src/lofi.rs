use std::f32::consts::PI;
use std::num::{NonZeroU16, NonZeroU32};

use anyhow::{Context, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

const SAMPLE_RATE: u32 = 44_100;
const BEATS_PER_BAR: f32 = 4.0;
const LOOP_BARS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    TapeBloom,
    NightDrive,
    RainStudy,
}

impl TrackKind {
    pub fn all() -> &'static [TrackKind] {
        &[Self::TapeBloom, Self::NightDrive, Self::RainStudy]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::TapeBloom => "Palo Alto Dawn",
            Self::NightDrive => "SoMa Afterhours",
            Self::RainStudy => "Shibuya Rain",
        }
    }

    fn bpm(self) -> f32 {
        match self {
            Self::TapeBloom => 78.0,
            Self::NightDrive => 92.0,
            Self::RainStudy => 68.0,
        }
    }

    fn gain(self) -> f32 {
        match self {
            Self::TapeBloom => 0.28,
            Self::NightDrive => 0.24,
            Self::RainStudy => 0.22,
        }
    }
}

#[derive(Clone, Copy)]
enum Voice {
    Pad,
    Bass,
    Lead,
    Pluck,
}

#[derive(Clone, Copy)]
struct NoteEvent {
    start_beat: f32,
    duration_beats: f32,
    midi_note: u8,
    velocity: f32,
    pan: f32,
    voice: Voice,
}

pub struct LofiPlayer {
    _sink: MixerDeviceSink,
    _player: Player,
    pub track: TrackKind,
}

impl LofiPlayer {
    pub fn start(track: TrackKind) -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .with_context(|| "failed to open a default audio output device")?;
        sink.log_on_drop(false);
        let player = Player::connect_new(&sink.mixer());
        let source = SamplesBuffer::new(
            NonZeroU16::new(2).expect("stereo channel count must be non-zero"),
            NonZeroU32::new(SAMPLE_RATE).expect("sample rate must be non-zero"),
            render_loop(track),
        )
        .amplify(track.gain())
        .repeat_infinite();
        player.append(source);
        Ok(Self {
            _sink: sink,
            _player: player,
            track,
        })
    }
}

fn midi_frequency(note: u8) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0)
}

fn bar_start(bar: usize) -> f32 {
    bar as f32 * BEATS_PER_BAR
}

fn build_events(track: TrackKind) -> Vec<NoteEvent> {
    match track {
        TrackKind::TapeBloom => build_tape_bloom(),
        TrackKind::NightDrive => build_night_drive(),
        TrackKind::RainStudy => build_rain_study(),
    }
}

fn build_tape_bloom() -> Vec<NoteEvent> {
    let chord_progression = [
        [57u8, 60u8, 64u8],
        [53u8, 57u8, 60u8],
        [50u8, 53u8, 57u8],
        [55u8, 59u8, 62u8],
        [57u8, 60u8, 64u8],
        [53u8, 57u8, 60u8],
        [52u8, 55u8, 59u8],
        [55u8, 59u8, 62u8],
    ];
    let bassline = [33u8, 29u8, 26u8, 31u8, 33u8, 29u8, 28u8, 31u8];
    let leadline = [
        (0.5f32, 72u8),
        (1.5, 74u8),
        (2.5, 76u8),
        (3.25, 74u8),
        (4.5, 69u8),
        (5.5, 71u8),
        (6.5, 72u8),
        (7.25, 71u8),
        (8.5, 69u8),
        (9.5, 67u8),
        (10.5, 69u8),
        (11.25, 71u8),
        (12.5, 72u8),
        (13.5, 74u8),
        (14.5, 76u8),
        (15.25, 74u8),
    ];

    let mut events = Vec::new();

    for (bar, chord) in chord_progression.iter().enumerate() {
        for (index, note) in chord.iter().enumerate() {
            events.push(NoteEvent {
                start_beat: bar_start(bar),
                duration_beats: 3.75,
                midi_note: *note,
                velocity: 0.18,
                pan: match index {
                    0 => -0.35,
                    1 => 0.0,
                    _ => 0.35,
                },
                voice: Voice::Pad,
            });
        }
    }

    for (bar, root) in bassline.iter().enumerate() {
        for step in 0..4 {
            let note = if step == 2 { *root + 7 } else { *root };
            events.push(NoteEvent {
                start_beat: bar_start(bar) + step as f32,
                duration_beats: if step == 3 { 0.75 } else { 0.9 },
                midi_note: note,
                velocity: if step == 0 { 0.28 } else { 0.2 },
                pan: -0.08,
                voice: Voice::Bass,
            });
        }
    }

    for (start_beat, note) in leadline {
        events.push(NoteEvent {
            start_beat,
            duration_beats: 0.75,
            midi_note: note,
            velocity: 0.14,
            pan: 0.18,
            voice: Voice::Lead,
        });
    }

    events
}

fn build_night_drive() -> Vec<NoteEvent> {
    let chords = [
        [45u8, 52u8, 57u8],
        [48u8, 55u8, 60u8],
        [43u8, 50u8, 55u8],
        [50u8, 57u8, 62u8],
        [45u8, 52u8, 57u8],
        [48u8, 55u8, 60u8],
        [47u8, 54u8, 59u8],
        [50u8, 57u8, 62u8],
    ];
    let bass = [33u8, 36u8, 31u8, 38u8, 33u8, 36u8, 35u8, 38u8];
    let plucks = [
        (0.0f32, 69u8),
        (0.75, 72u8),
        (1.5, 76u8),
        (2.25, 72u8),
        (4.0, 67u8),
        (4.75, 71u8),
        (5.5, 74u8),
        (6.25, 71u8),
        (8.0, 66u8),
        (8.75, 69u8),
        (9.5, 73u8),
        (10.25, 69u8),
        (12.0, 67u8),
        (12.75, 71u8),
        (13.5, 74u8),
        (14.25, 78u8),
    ];

    let mut events = Vec::new();

    for (bar, chord) in chords.iter().enumerate() {
        for (index, note) in chord.iter().enumerate() {
            events.push(NoteEvent {
                start_beat: bar_start(bar),
                duration_beats: 3.9,
                midi_note: *note,
                velocity: 0.16,
                pan: if index == 0 {
                    -0.28
                } else if index == 1 {
                    0.0
                } else {
                    0.28
                },
                voice: Voice::Pad,
            });
        }
    }

    for (bar, note) in bass.iter().enumerate() {
        for step in 0..8 {
            let offset = step as f32 * 0.5;
            events.push(NoteEvent {
                start_beat: bar_start(bar) + offset,
                duration_beats: 0.38,
                midi_note: if step % 4 == 3 { *note + 7 } else { *note },
                velocity: if step % 4 == 0 { 0.23 } else { 0.14 },
                pan: -0.03,
                voice: Voice::Bass,
            });
        }
    }

    for (start_beat, note) in plucks {
        events.push(NoteEvent {
            start_beat,
            duration_beats: 0.45,
            midi_note: note,
            velocity: 0.13,
            pan: 0.22,
            voice: Voice::Pluck,
        });
    }

    events
}

fn build_rain_study() -> Vec<NoteEvent> {
    let chords = [
        [50u8, 57u8, 62u8],
        [48u8, 55u8, 60u8],
        [45u8, 52u8, 57u8],
        [47u8, 54u8, 59u8],
        [50u8, 57u8, 62u8],
        [48u8, 55u8, 60u8],
        [43u8, 50u8, 55u8],
        [47u8, 54u8, 59u8],
    ];
    let bass = [26u8, 24u8, 21u8, 23u8, 26u8, 24u8, 19u8, 23u8];
    let lead = [
        (1.0f32, 74u8),
        (3.0, 72u8),
        (5.0, 71u8),
        (7.0, 69u8),
        (9.0, 74u8),
        (11.0, 76u8),
        (13.0, 72u8),
        (15.0, 71u8),
    ];

    let mut events = Vec::new();

    for (bar, chord) in chords.iter().enumerate() {
        for (index, note) in chord.iter().enumerate() {
            events.push(NoteEvent {
                start_beat: bar_start(bar),
                duration_beats: 4.0,
                midi_note: *note,
                velocity: 0.14,
                pan: match index {
                    0 => -0.32,
                    1 => 0.0,
                    _ => 0.32,
                },
                voice: Voice::Pad,
            });
        }
    }

    for (bar, note) in bass.iter().enumerate() {
        events.push(NoteEvent {
            start_beat: bar_start(bar),
            duration_beats: 1.8,
            midi_note: *note,
            velocity: 0.2,
            pan: -0.06,
            voice: Voice::Bass,
        });
        events.push(NoteEvent {
            start_beat: bar_start(bar) + 2.0,
            duration_beats: 1.5,
            midi_note: *note + 7,
            velocity: 0.14,
            pan: -0.02,
            voice: Voice::Bass,
        });
    }

    for (start_beat, note) in lead {
        events.push(NoteEvent {
            start_beat,
            duration_beats: 1.5,
            midi_note: note,
            velocity: 0.11,
            pan: 0.18,
            voice: Voice::Lead,
        });
    }

    events
}

fn render_loop(track: TrackKind) -> Vec<f32> {
    let bpm = track.bpm();
    let seconds_per_beat = 60.0 / bpm;
    let total_beats = LOOP_BARS as f32 * BEATS_PER_BAR;
    let total_seconds = total_beats * seconds_per_beat;
    let total_frames = (total_seconds * SAMPLE_RATE as f32) as usize;
    let events = build_events(track);
    let mut samples = Vec::with_capacity(total_frames * 2);

    for frame in 0..total_frames {
        let time = frame as f32 / SAMPLE_RATE as f32;
        let beat = time / seconds_per_beat;
        let wobble = match track {
            TrackKind::TapeBloom => (2.0 * PI * 0.18 * time).sin() * 0.0025,
            TrackKind::NightDrive => (2.0 * PI * 0.12 * time).sin() * 0.0012,
            TrackKind::RainStudy => (2.0 * PI * 0.09 * time).sin() * 0.0035,
        };
        let flutter = match track {
            TrackKind::TapeBloom => (2.0 * PI * 3.6 * time).sin() * 0.0007,
            TrackKind::NightDrive => (2.0 * PI * 2.2 * time).sin() * 0.0003,
            TrackKind::RainStudy => (2.0 * PI * 2.8 * time).sin() * 0.0005,
        };
        let pitch_mod = 1.0 + wobble + flutter;
        let mut left = 0.0f32;
        let mut right = 0.0f32;

        for event in &events {
            if beat < event.start_beat || beat > event.start_beat + event.duration_beats {
                continue;
            }
            let local_time = (beat - event.start_beat) * seconds_per_beat;
            let duration = event.duration_beats * seconds_per_beat;
            let frequency = midi_frequency(event.midi_note) * pitch_mod;
            let amplitude =
                note_envelope(track, event.voice, local_time, duration) * event.velocity;
            let voice = voice_sample(track, event.voice, frequency, local_time, amplitude);
            let pan_left = ((1.0 - event.pan).clamp(0.0, 2.0)) * 0.5;
            let pan_right = ((1.0 + event.pan).clamp(0.0, 2.0)) * 0.5;
            left += voice * pan_left;
            right += voice * pan_right;
        }

        let ambience = ambient_layer(track, time);
        left = soft_clip(left + ambience.0);
        right = soft_clip(right + ambience.1);

        samples.push(left);
        samples.push(right);
    }

    samples
}

fn note_envelope(track: TrackKind, voice: Voice, time: f32, duration: f32) -> f32 {
    let (attack, release, sustain_floor) = match (track, voice) {
        (_, Voice::Bass) => (0.012, 0.12, 0.86),
        (TrackKind::NightDrive, Voice::Pluck) => (0.004, 0.14, 0.45),
        (TrackKind::RainStudy, Voice::Lead) => (0.08, 0.4, 0.72),
        (_, Voice::Lead) => (0.02, 0.24, 0.7),
        _ => (0.03, 0.28, 0.82),
    };
    let attack_phase = (time / attack).clamp(0.0, 1.0);
    let release_phase = ((duration - time) / release).clamp(0.0, 1.0);
    let sustain = sustain_floor + (1.0 - sustain_floor) * (1.0 - (time / duration).clamp(0.0, 1.0));
    attack_phase * release_phase * sustain
}

fn voice_sample(track: TrackKind, voice: Voice, frequency: f32, time: f32, amplitude: f32) -> f32 {
    let fundamental = (2.0 * PI * frequency * time).sin();
    let octave = (2.0 * PI * frequency * 2.0 * time).sin();
    let fifth = (2.0 * PI * frequency * 3.0 * time).sin();
    let noise = (2.0 * PI * frequency * 0.5 * time).sin().signum() * 0.04;

    match (track, voice) {
        (_, Voice::Pad) => {
            amplitude
                * (0.68 * fundamental + 0.2 * octave + 0.08 * fifth + 0.04 * noise)
                * (1.0 - (time * 0.12).min(0.5))
        }
        (_, Voice::Bass) => amplitude * (0.9 * fundamental + 0.1 * octave.abs().copysign(octave)),
        (TrackKind::NightDrive, Voice::Pluck) => {
            amplitude * (0.62 * fundamental + 0.24 * octave + 0.14 * fifth)
        }
        (TrackKind::RainStudy, Voice::Lead) => {
            amplitude * (0.74 * fundamental + 0.12 * octave + 0.14 * (frequency * time * PI).sin())
        }
        (_, Voice::Lead) => {
            amplitude * (0.58 * fundamental + 0.22 * octave + 0.20 * (frequency * time * PI).sin())
        }
        (_, Voice::Pluck) => amplitude * (0.72 * fundamental + 0.28 * octave),
    }
}

fn ambient_layer(track: TrackKind, time: f32) -> (f32, f32) {
    match track {
        TrackKind::TapeBloom => {
            let hum = 0.006 * (2.0 * PI * 60.0 * time).sin();
            let drift = 0.003 * (2.0 * PI * 0.11 * time).sin();
            (hum + drift, hum - drift)
        }
        TrackKind::NightDrive => {
            let neon = 0.003 * (2.0 * PI * 48.0 * time).sin();
            let air = 0.002 * (2.0 * PI * 0.07 * time).sin();
            (neon + air, neon - air)
        }
        TrackKind::RainStudy => {
            let rain_left = 0.004 * (2.0 * PI * 13.0 * time).sin() * (2.0 * PI * 0.17 * time).sin();
            let rain_right =
                0.004 * (2.0 * PI * 11.0 * time).sin() * (2.0 * PI * 0.19 * time).sin();
            (rain_left, rain_right)
        }
    }
}

fn soft_clip(sample: f32) -> f32 {
    (sample * 1.35).tanh() * 0.72
}
