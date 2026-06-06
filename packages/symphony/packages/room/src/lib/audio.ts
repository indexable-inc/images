// Mic capture + Opus encode → WebTransport datagrams, and the inverse
// on receive. Always-on mesh: every peer in the room hears every
// other peer.
//
// Capture path
//   getUserMedia → AudioWorkletNode('room-capture') → 20 ms Float32
//   frames → WebCodecs `AudioEncoder({ codec: 'opus' })` → raw Opus
//   packets → `transport.sendDatagram(packet)`.
//
// Playback path
//   transport datagram (8-byte peer id prefix + Opus) → strip prefix
//   → per-peer `AudioDecoder` → main-thread mix → single
//   AudioWorkletNode('room-playback') → AudioContext.destination.
//
// The server stamps the sender id on every datagram it relays and
// drops self-echoes, so this module never needs to know its own id.
//
// Hardware sample rate (typically 48 kHz on macOS) is whatever the
// AudioContext picks; Opus accepts 8/12/16/24/48 kHz natively and the
// encoder is configured at that rate, so no resampling is needed.
//
// Bad fit if: WebCodecs `AudioEncoder` is unavailable (older
// WebKit/WKWebView, certain Linux WebViews). Capture silently
// no-ops; the receive path still mixes peers that do encode, so a
// listen-only client works.

import type { RoomTransport } from './transport';

const FRAME_MS = 20;
const TARGET_BITRATE = 16000;

const AUDIO_PEER_ID_LEN = 8;

interface PeerDecoder {
  decoder: AudioDecoder;
  sampleRate: number;
}

export interface VoiceController {
  /** Tear down both directions. Closes the AudioContext, releases
   *  the mic, abandons in-flight encoder/decoder work. Safe to call
   *  even if start failed. */
  stop(): void;
}

/** Start the voice loop on this peer. Captures the mic if permission
 *  is granted; always opens the receive path so we hear other peers
 *  even when we have no input device.
 *
 *  Returns a controller whose `stop()` releases all resources. */
export async function startVoice(transport: RoomTransport): Promise<VoiceController> {
  if (typeof AudioContext === 'undefined') {
    throw new Error('AudioContext unavailable in this environment');
  }
  if (typeof AudioEncoder === 'undefined' || typeof AudioDecoder === 'undefined') {
    throw new Error(
      'WebCodecs Opus encode/decode unavailable — needs a recent browser'
    );
  }

  const ctx = new AudioContext({ latencyHint: 'interactive' });
  await ctx.audioWorklet.addModule('/audio-worklets/capture-processor.js');
  await ctx.audioWorklet.addModule('/audio-worklets/playback-processor.js');

  const sampleRate = ctx.sampleRate;
  const frameSamples = Math.round((sampleRate * FRAME_MS) / 1000);

  // Playback path. One mixed mono stream; per-peer decoders push
  // PCM into the same worklet ring buffer with sample-level offset
  // bookkeeping done on the main thread.
  const playback = new AudioWorkletNode(ctx, 'room-playback', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [1],
    processorOptions: { capacity: sampleRate } // 1s
  });
  playback.connect(ctx.destination);

  playback.port.onmessage = (ev) => {
    const data = ev.data;
    if (data && data.type === 'stats' && data.underflows > 0) {
      console.debug(
        'room: playback underflows in last 1s',
        data.underflows,
        'available samples',
        data.available
      );
    }
  };

  const peerDecoders = new Map<string, PeerDecoder>();

  function decoderFor(peerKey: string): PeerDecoder {
    const existing = peerDecoders.get(peerKey);
    if (existing) return existing;
    const decoder = new AudioDecoder({
      output: (data) => {
        // AudioData → planar Float32, mono, mixed into the worklet.
        const channels = Math.min(data.numberOfChannels, 1);
        const frames = data.numberOfFrames;
        if (channels === 0 || frames === 0) {
          data.close();
          return;
        }
        const pcm = new Float32Array(frames);
        data.copyTo(pcm, { planeIndex: 0, format: 'f32-planar' });
        data.close();
        playback.port.postMessage(pcm, [pcm.buffer]);
      },
      error: (err) => {
        console.warn('room: opus decode error from peer', peerKey, err);
      }
    });
    decoder.configure({ codec: 'opus', sampleRate, numberOfChannels: 1 });
    const created = { decoder, sampleRate };
    peerDecoders.set(peerKey, created);
    return created;
  }

  const offDatagram = transport.onDatagram((bytes) => {
    if (bytes.byteLength <= AUDIO_PEER_ID_LEN) return;
    const peerKey = bytesToHex(bytes.subarray(0, AUDIO_PEER_ID_LEN));
    // slice() gives a Uint8Array<ArrayBuffer> we own, both for the
    // type contract on `EncodedAudioChunk.data` and so the decoder
    // can keep the packet past the next transport read.
    const opus = bytes.slice(AUDIO_PEER_ID_LEN);
    const peer = decoderFor(peerKey);
    if (peer.decoder.state !== 'configured') return;
    try {
      peer.decoder.decode(
        new EncodedAudioChunk({
          type: 'key',
          timestamp: performance.now() * 1000,
          data: opus
        })
      );
    } catch (err) {
      console.warn('room: failed to enqueue decode', err);
    }
  });

  // Capture path. Best-effort: a denied mic prompt or a failure to
  // configure the encoder just logs and leaves the listen-only loop
  // in place.
  let stream: MediaStream | null = null;
  let captureNode: AudioWorkletNode | null = null;
  let encoder: AudioEncoder | null = null;
  let mediaSourceNode: MediaStreamAudioSourceNode | null = null;
  let timestamp = 0;

  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true
      }
    });

    encoder = new AudioEncoder({
      output: (chunk) => {
        const buf = new Uint8Array(chunk.byteLength);
        chunk.copyTo(buf);
        transport.sendDatagram(buf);
      },
      error: (err) => {
        console.warn('room: opus encode error', err);
      }
    });
    encoder.configure({
      codec: 'opus',
      sampleRate,
      numberOfChannels: 1,
      bitrate: TARGET_BITRATE,
      bitrateMode: 'constant'
    } as AudioEncoderConfig);

    mediaSourceNode = ctx.createMediaStreamSource(stream);
    captureNode = new AudioWorkletNode(ctx, 'room-capture', {
      numberOfInputs: 1,
      numberOfOutputs: 0,
      processorOptions: { sampleRate }
    });
    mediaSourceNode.connect(captureNode);

    captureNode.port.onmessage = (ev) => {
      const frame = ev.data;
      if (!(frame instanceof Float32Array)) return;
      if (!encoder || encoder.state !== 'configured') return;
      // Re-back the samples by a fresh ArrayBuffer so the AudioData
      // constructor's BufferSource type contract is satisfied
      // (TypeScript's strict bufferview typing rejects the transfer
      // result's `Float32Array<ArrayBufferLike>` directly).
      const owned = new Float32Array(frame.length);
      owned.set(frame);
      const data = new AudioData({
        format: 'f32-planar',
        sampleRate,
        numberOfFrames: frameSamples,
        numberOfChannels: 1,
        timestamp,
        data: owned
      });
      timestamp += Math.round((frameSamples * 1_000_000) / sampleRate);
      try {
        encoder.encode(data);
      } catch (err) {
        console.warn('room: encoder.encode failed', err);
      }
      data.close();
    };
  } catch (err) {
    console.warn('room: voice capture unavailable, going listen-only', err);
  }

  return {
    stop() {
      offDatagram();
      try {
        captureNode?.disconnect();
        mediaSourceNode?.disconnect();
        playback.disconnect();
      } catch {
        // ignore
      }
      if (encoder && encoder.state === 'configured') encoder.close();
      for (const peer of peerDecoders.values()) {
        if (peer.decoder.state === 'configured') peer.decoder.close();
      }
      peerDecoders.clear();
      if (stream) {
        for (const track of stream.getTracks()) track.stop();
      }
      void ctx.close();
    }
  };
}

function bytesToHex(bytes: Uint8Array): string {
  let out = '';
  for (const b of bytes) out += b.toString(16).padStart(2, '0');
  return out;
}
