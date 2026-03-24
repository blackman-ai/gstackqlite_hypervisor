use std::f32::consts::PI;
use std::num::{NonZeroU16, NonZeroU32};

use anyhow::{Context, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

const SAMPLE_RATE: u32 = 44_100;
const BPM: f32 = 78.0;
const BEATS_PER_BAR: f32 = 4.0;
const LOOP_BARS: usize = 8;

#[derive(Clone, Copy)]
enum Voice {
    Pad,
    Bass,
    Lead,
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
}

impl LofiPlayer {
    pub fn start() -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .with_context(|| "failed to open a default audio output device")?;
        sink.log_on_drop(false);
        let player = Player::connect_new(&sink.mixer());
        let source = SamplesBuffer::new(
            NonZeroU16::new(2).expect("stereo channel count must be non-zero"),
            NonZeroU32::new(SAMPLE_RATE).expect("sample rate must be non-zero"),
            render_loop(),
        )
        .amplify(0.28)
        .repeat_infinite();
        player.append(source);
        Ok(Self {
            _sink: sink,
            _player: player,
        })
    }
}

fn midi_frequency(note: u8) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0)
}

fn bar_start(bar: usize) -> f32 {
    bar as f32 * BEATS_PER_BAR
}

fn build_events() -> Vec<NoteEvent> {
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

fn render_loop() -> Vec<f32> {
    let seconds_per_beat = 60.0 / BPM;
    let total_beats = LOOP_BARS as f32 * BEATS_PER_BAR;
    let total_seconds = total_beats * seconds_per_beat;
    let total_frames = (total_seconds * SAMPLE_RATE as f32) as usize;
    let events = build_events();
    let mut samples = Vec::with_capacity(total_frames * 2);

    for frame in 0..total_frames {
        let time = frame as f32 / SAMPLE_RATE as f32;
        let beat = time / seconds_per_beat;
        let wobble = (2.0 * PI * 0.18 * time).sin() * 0.0025;
        let flutter = (2.0 * PI * 3.6 * time).sin() * 0.0007;
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
            let amplitude = note_envelope(local_time, duration) * event.velocity;
            let voice = voice_sample(event.voice, frequency, local_time, amplitude);
            let pan_left = ((1.0 - event.pan).clamp(0.0, 2.0)) * 0.5;
            let pan_right = ((1.0 + event.pan).clamp(0.0, 2.0)) * 0.5;
            left += voice * pan_left;
            right += voice * pan_right;
        }

        let tape_hum = 0.006 * (2.0 * PI * 60.0 * time).sin();
        let drift = 0.003 * (2.0 * PI * 0.11 * time).sin();
        left = soft_clip(left + tape_hum + drift);
        right = soft_clip(right + tape_hum - drift);

        samples.push(left);
        samples.push(right);
    }

    samples
}

fn note_envelope(time: f32, duration: f32) -> f32 {
    let attack = 0.025;
    let release = 0.24;
    let attack_phase = (time / attack).clamp(0.0, 1.0);
    let release_phase = ((duration - time) / release).clamp(0.0, 1.0);
    let sustain = if duration > release {
        (time / duration).clamp(0.0, 1.0)
    } else {
        1.0
    };
    attack_phase * release_phase * (0.82 + 0.18 * (1.0 - sustain))
}

fn voice_sample(voice: Voice, frequency: f32, time: f32, amplitude: f32) -> f32 {
    let fundamental = (2.0 * PI * frequency * time).sin();
    let octave = (2.0 * PI * frequency * 2.0 * time).sin();
    let fifth = (2.0 * PI * frequency * 3.0 * time).sin();
    match voice {
        Voice::Pad => {
            amplitude
                * (0.72 * fundamental + 0.18 * octave + 0.10 * fifth)
                * (1.0 - (time * 0.18).min(0.55))
        }
        Voice::Bass => amplitude * (0.88 * fundamental + 0.12 * octave.signum() * octave.abs()),
        Voice::Lead => {
            amplitude * (0.58 * fundamental + 0.22 * octave + 0.20 * (frequency * time * PI).sin())
        }
    }
}

fn soft_clip(sample: f32) -> f32 {
    (sample * 1.35).tanh() * 0.72
}
