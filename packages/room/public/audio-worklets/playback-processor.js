// AudioWorklet that drains a shared Float32 ring buffer of mixed PCM
// the main thread fills as Opus packets decode. One processor renders
// the merged voice of every remote peer; mixing happens on the main
// thread so the audio thread stays a pure consumer.
//
// Underflow inserts silence (zero quanta) and bumps a drop counter
// reported back to the main thread so the jitter target can adapt.
// Overflow drops the oldest unread frame, on the theory that fresh
// audio always wins over stale.

class PlaybackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = options?.processorOptions ?? {};
    this._capacity = opts.capacity ?? 48000; // 1s @ 48kHz fallback
    this._buffer = new Float32Array(this._capacity);
    this._read = 0;
    this._write = 0;
    this._available = 0;
    this._underflows = 0;
    this._lastReport = currentTime;

    this.port.onmessage = (ev) => {
      const data = ev.data;
      if (data instanceof Float32Array) {
        this._enqueue(data);
      } else if (data && data.type === 'reset') {
        this._read = this._write = this._available = 0;
      }
    };
  }

  _enqueue(samples) {
    const cap = this._capacity;
    for (let i = 0; i < samples.length; i++) {
      this._buffer[this._write] = samples[i];
      this._write = (this._write + 1) % cap;
      if (this._available < cap) {
        this._available++;
      } else {
        // overflow: advance read so write stays ahead
        this._read = (this._read + 1) % cap;
      }
    }
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0) return true;
    const channel = output[0];
    const need = channel.length;

    if (this._available < need) {
      this._underflows++;
      channel.fill(0);
    } else {
      for (let i = 0; i < need; i++) {
        channel[i] = this._buffer[this._read];
        this._read = (this._read + 1) % this._capacity;
      }
      this._available -= need;
    }
    // Mirror to other output channels if any.
    for (let c = 1; c < output.length; c++) {
      output[c].set(channel);
    }

    if (currentTime - this._lastReport > 1.0) {
      this.port.postMessage({
        type: 'stats',
        underflows: this._underflows,
        available: this._available
      });
      this._underflows = 0;
      this._lastReport = currentTime;
    }
    return true;
  }
}

registerProcessor('room-playback', PlaybackProcessor);
