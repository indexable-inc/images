// AudioWorklet that hands the main thread fixed 20 ms PCM frames from
// the microphone. The worklet runs on the audio rendering thread, so
// the only allocation per render quantum is a small index update; the
// frame buffer is reused.
//
// 20 ms at the AudioContext sample rate (typically 48 kHz on macOS) is
// 960 samples per channel. The processor posts one Float32Array per
// frame to the main thread, which feeds the WebCodecs Opus encoder.

const FRAME_MS = 20;

class CaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = options?.processorOptions ?? {};
    const sampleRate = opts.sampleRate ?? sampleRate;
    this._frameSamples = Math.round((sampleRate * FRAME_MS) / 1000);
    this._buffer = new Float32Array(this._frameSamples);
    this._offset = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;
    const channel = input[0];
    if (!channel) return true;

    let read = 0;
    while (read < channel.length) {
      const remaining = this._frameSamples - this._offset;
      const take = Math.min(remaining, channel.length - read);
      this._buffer.set(channel.subarray(read, read + take), this._offset);
      this._offset += take;
      read += take;
      if (this._offset === this._frameSamples) {
        // Transfer a copy so the worklet's reusable buffer is free
        // to fill the next frame while the main thread encodes.
        const frame = new Float32Array(this._frameSamples);
        frame.set(this._buffer);
        this.port.postMessage(frame, [frame.buffer]);
        this._offset = 0;
      }
    }
    return true;
  }
}

registerProcessor('room-capture', CaptureProcessor);
