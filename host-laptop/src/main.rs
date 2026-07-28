use std::io::{self, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use engine::Oscillator;

/// Conservative fixed volume so the first tone isn't startlingly loud.
const AMPLITUDE: f32 = 0.2;
const FREQUENCY_HZ: f32 = 440.0; // A4

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default output device found")?;
    let supported_config = device.default_output_config()?;

    println!("Output device: {device}");
    println!(
        "Sample format: {:?}, sample rate: {} Hz, channels: {}",
        supported_config.sample_format(),
        supported_config.sample_rate(),
        supported_config.channels()
    );

    // Whether the tone should currently be audible. Shared between this (main) thread,
    // which flips it on Enter, and the audio callback thread, which only ever reads it.
    // An AtomicBool never makes the audio thread wait for a lock, unlike a Mutex would -
    // and the audio thread must never block, or the sound card's buffer runs dry and you
    // hear a click.
    let is_on = Arc::new(AtomicBool::new(false));
    let stream_config = supported_config.config();

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, stream_config, is_on.clone())?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, stream_config, is_on.clone())?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, stream_config, is_on.clone())?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;

    println!("Press Enter to toggle the {FREQUENCY_HZ} Hz tone on/off. Type 'q' then Enter to quit.");
    for line in io::stdin().lock().lines() {
        if line?.trim() == "q" {
            break;
        }
        let now_on = !is_on.load(Ordering::Relaxed);
        is_on.store(now_on, Ordering::Relaxed);
        println!("tone: {}", if now_on { "on" } else { "off" });
    }

    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    is_on: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut oscillator = Oscillator::new(config.sample_rate as f32, FREQUENCY_HZ);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
            for frame in data.chunks_mut(channels) {
                let sample = if is_on.load(Ordering::Relaxed) {
                    oscillator.next_sample() * AMPLITUDE
                } else {
                    0.0
                };
                let value = T::from_sample(sample);
                for out in frame.iter_mut() {
                    *out = value;
                }
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    Ok(stream)
}
