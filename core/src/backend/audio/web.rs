use super::{AudioBackend, AudioStreamHandle};
use generational_arena::Arena;
use js_sys::{ArrayBuffer, Uint8Array};
use log::info;
use swf::SoundStreamInfo;
use wasm_bindgen::closure::Closure;
use web_sys::{AudioBuffer, AudioBufferSourceNode, AudioContext};

pub struct WebAudioBackend {
    context: AudioContext,
    streams: Arena<AudioStream>,
}

struct AudioStream {
    info: SoundStreamInfo,
    time: f64,
    cur_mp3_frames: Vec<u8>,
    num_mp3_frames: usize,
    old_mp3_frames: Vec<Vec<u8>>,
}

impl WebAudioBackend {
    pub fn new() -> Result<Self, Box<std::error::Error>> {
        let context = AudioContext::new().map_err(|_| "Unable to create AudioContext")?;
        Ok(Self {
            context,
            streams: Arena::new(),
        })
    }

    const BUFFER_TIME: f64 = 0.05;
}

impl AudioBackend for WebAudioBackend {
    fn register_stream(&mut self, stream_info: &swf::SoundStreamInfo) -> AudioStreamHandle {
        let stream = AudioStream {
            info: stream_info.clone(),
            time: 0.0,
            cur_mp3_frames: vec![],
            old_mp3_frames: vec![],
            num_mp3_frames: 0,
        };
        info!("Stream {}", stream_info.num_samples_per_block);
        self.streams.insert(stream)
    }

    fn queue_stream_samples(&mut self, handle: AudioStreamHandle, samples: &[u8]) {
        if let Some(stream) = self.streams.get_mut(handle) {
            let current_time = self.context.current_time();
            if current_time >= stream.time {
                stream.time = current_time + WebAudioBackend::BUFFER_TIME;
            }

            let format = &stream.info.stream_format;

            let num_channels = if format.is_stereo { 2 } else { 1 };
            let frame_size = num_channels * if format.is_16_bit { 2 } else { 1 };
            let num_frames = samples.len() / frame_size;

            if format.compression == swf::AudioCompression::Uncompressed
                || format.compression == swf::AudioCompression::UncompressedUnknownEndian
            {
                let buffer = self
                    .context
                    .create_buffer(
                        num_channels as u32,
                        num_frames as u32,
                        format.sample_rate.into(),
                    )
                    .unwrap();

                let mut i = 0;

                if num_channels == 2 {
                    let mut left_samples = Vec::with_capacity(num_frames);
                    let mut right_samples = Vec::with_capacity(num_frames);

                    if format.is_16_bit {
                        while i < num_frames * 4 {
                            let left_sample =
                                ((samples[i] as u16) | ((samples[i + 1] as u16) << 8)) as i16;
                            let right_sample =
                                ((samples[i + 2] as u16) | ((samples[i + 3] as u16) << 8)) as i16;
                            left_samples.push((f32::from(left_sample)) / 32768.0);
                            right_samples.push((f32::from(right_sample)) / 32768.0);
                            i += 4;
                        }
                    } else {
                        while i < num_frames * 2 {
                            left_samples.push((f32::from(samples[i]) - 127.0) / 128.0);
                            right_samples.push((f32::from(samples[i + 1]) - 127.0) / 128.0);
                            i += 2;
                        }
                    }
                    buffer.copy_to_channel(&mut left_samples[..], 0).unwrap();
                    buffer.copy_to_channel(&mut right_samples[..], 1).unwrap();
                } else {
                    let mut out_samples = Vec::with_capacity(num_frames);
                    if format.is_16_bit {
                        while i < num_frames * 2 {
                            let sample = f32::from(
                                ((samples[i] as u16) | ((samples[i + 1] as u16) << 8)) as i16,
                            ) / 32768.0;
                            if i == 0 {
                                info!("S: {}", sample);
                            }
                            out_samples.push(sample);
                            i += 2;
                        }
                    } else {
                        while i < num_frames {
                            out_samples.push((f32::from(samples[i]) - 127.0) / 128.0);
                            i += 1;
                        }
                    }

                    buffer.copy_to_channel(&mut out_samples[..], 0).unwrap();
                }

                let buffer_node = self.context.create_buffer_source().unwrap();
                buffer_node.set_buffer(Some(&buffer));
                buffer_node
                    .connect_with_audio_node(&self.context.destination())
                    .unwrap();

                buffer_node.start_with_when(stream.time).unwrap();

                stream.time += (num_frames as f64) / (format.sample_rate as f64);
            } else if format.compression == swf::AudioCompression::Mp3 {
                let num_frames = ((samples[0] as u16) | ((samples[1] as u16) << 8)) as usize;
                let num_frames_to_skip =
                    ((samples[2] as u16) | ((samples[3] as u16) << 8)) as usize;
                info!("play {} {}", num_frames, num_frames_to_skip);
                if num_frames == 0 {
                    return;
                }

                // let mut mp3_samples = vec![];
                // mp3_samples.extend(b"ID3".iter());
                // mp3_samples.push(0);
                // mp3_samples.push(0);
                // mp3_samples.push(0);
                // mp3_samples.push(10);
                // mp3_samples.push(0);
                // mp3_samples.push(0);
                // mp3_samples.push(0);
                stream.cur_mp3_frames.extend(samples.iter().skip(4));
                stream.num_mp3_frames += num_frames;
                // let data_array = unsafe { Uint8Array::view(&clone_samples[..]) };

                if stream.cur_mp3_frames.len() >= 576 * 4 {
                    stream.old_mp3_frames.push(stream.cur_mp3_frames.clone());
                    let mp3_frames = &stream.old_mp3_frames[stream.old_mp3_frames.len() - 1];
                    let data_array = unsafe { Uint8Array::view(&mp3_frames[..]) };
                    let array_buffer = data_array.buffer().slice_with_end(
                        data_array.byte_offset(),
                        data_array.byte_offset() + data_array.byte_length(),
                    );
                    //data_array.buffer();
                    //info!("{} {:?}", mp3_samples.len(), array_buffer.byte_length());
                    let context = self.context.clone();
                    let n = stream.time;
                    let closure = Closure::wrap(Box::new(move |buffer: wasm_bindgen::JsValue| {
                        let buffer_node = context.create_buffer_source().unwrap();
                        buffer_node.set_buffer(Some(&buffer.into()));
                        buffer_node
                            .connect_with_audio_node(&context.destination())
                            .unwrap();

                        buffer_node.start_with_when(n).unwrap();
                    })
                        as Box<dyn FnMut(wasm_bindgen::JsValue)>);
                    self.context
                        .decode_audio_data(&array_buffer)
                        .unwrap()
                        .then(&closure);
                    closure.forget();

                    stream.time += (stream.num_mp3_frames as f64) / (format.sample_rate as f64);
                    stream.num_mp3_frames = 0;
                    stream.cur_mp3_frames.clear();
                }
                // let num_frames = ((samples[0] as u16) | ((samples[1] as u16) << 8)) as usize;

                // let buffer = self
                //     .context
                //     .create_buffer(
                //         num_channels as u32,
                //         num_frames as u32,
                //         format.sample_rate.into(),
                //     )
                //     .unwrap();

                if format.is_stereo {
                    // let mut left_samples = Vec::with_capacity(num_frames);
                    // let mut right_samples = Vec::with_capacity(num_frames);

                    // use minimp3::{Decoder, Error, Frame};
                    // let mut decoder = Decoder::new(&samples[2..]);
                    // let mut frames_decoded = 0;
                    // while frames_decoded < num_frames {
                    //     match decoder.next_frame() {
                    //         Ok(Frame {
                    //             data,
                    //             sample_rate,
                    //             channels,
                    //             ..
                    //         }) => {
                    //             let new_frames_decoded = data.len() / channels;
                    //             frames_decoded += new_frames_decoded;

                    //             let mut i: usize = 0;
                    //             while i < new_frames_decoded {
                    //                 let left_sample = data[i];
                    //                 let right_sample = data[i + 1];
                    //                 left_samples.push((f32::from(left_sample)) / 32768.0);
                    //                 right_samples.push((f32::from(right_sample)) / 32768.0);
                    //                 i += 2;
                    //             }
                    //         }
                    //         Err(Error::Eof) => {
                    //             frames_decoded = num_frames;
                    //             break;
                    //         }
                    //         Err(e) => panic!("{:?}", e),
                    //     }
                    //     buffer.copy_to_channel(&mut left_samples[..], 0).unwrap();
                    //     buffer.copy_to_channel(&mut right_samples[..], 1).unwrap();
                    // }
                }
            }
        }
    }
}