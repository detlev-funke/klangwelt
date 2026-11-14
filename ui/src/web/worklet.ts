// Web shell, AudioWorklet: this is where the engine's renderer mixes
// (`klang_worklet.wasm`) – the counterpart to the desktop's audio callback.
// Messages and sound buffers come from the web core (worker) via a
// MessageChannel; status and microphone samples go back there. No
// SharedArrayBuffer needed (a simple server does not set COOP/COEP).

declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: unknown);
}
declare function registerProcessor(name: string, ctor: new (options: { processorOptions: { wasm: ArrayBuffer } }) => AudioWorkletProcessor): void;
declare const sampleRate: number;

interface WorkletExports {
  memory: WebAssembly.Memory;
  kwl_init(rate: number): void;
  kwl_msg_buf(n: number): number;
  kwl_msg(kind: number, n: number): void;
  kwl_buffer_alloc(id: number, len: number): number;
  kwl_buffer_done(id: number): void;
  kwl_process(frames: number): number;
  kwl_status(): number;
}

type FromWorker = { k: number; d: Float64Array } | { b: number; n: number; off: number; d: Float32Array } | { flush: number };

/** `klang_engine::wire::Kind::Record` – this script collects the recording. */
const RECORD = 13;
/** Status every this many blocks (128 frames: about 24 ms at 48 kHz). */
const STATUS_EVERY = 9;
/** Microphone samples per message to the worker. */
const REC_CHUNK = 4096;

class KlangweltProcessor extends AudioWorkletProcessor {
  private wasm: WorkletExports;
  private worker: MessagePort | null = null;
  /** Buffers under construction: id → pointer. */
  private filling = new Map<number, number>();
  private recording = false;
  private rec = new Float32Array(REC_CHUNK);
  private recPos = 0;
  private peakIn = 0;
  private blocks = 0;

  constructor(options: { processorOptions: { wasm: ArrayBuffer } }) {
    super();
    const module = new WebAssembly.Module(options.processorOptions.wasm);
    this.wasm = new WebAssembly.Instance(module, {}).exports as unknown as WorkletExports;
    this.wasm.kwl_init(sampleRate);
    this.port.onmessage = (e: MessageEvent<{ port?: MessagePort }>) => {
      if (!e.data.port) return;
      this.worker = e.data.port;
      this.worker.onmessage = (m: MessageEvent<FromWorker>) => this.fromWorker(m.data);
    };
  }

  private fromWorker(m: FromWorker): void {
    if ("k" in m) {
      if (m.k === RECORD) {
        const on = m.d[0] !== 0;
        if (!on) this.sendRecording();
        this.recording = on;
        this.recPos = 0;
      }
      const ptr = this.wasm.kwl_msg_buf(m.d.length);
      new Float64Array(this.wasm.memory.buffer, ptr, m.d.length).set(m.d);
      this.wasm.kwl_msg(m.k, m.d.length);
    } else if ("b" in m) {
      // A buffer arrives in pieces; only when complete is it usable.
      if (m.off === 0) this.filling.set(m.b, this.wasm.kwl_buffer_alloc(m.b, m.n));
      const ptr = this.filling.get(m.b);
      if (ptr == null) return;
      if (m.d.length) new Float32Array(this.wasm.memory.buffer, ptr + m.off * 4, m.d.length).set(m.d);
      if (m.off + m.d.length >= m.n) {
        this.wasm.kwl_buffer_done(m.b);
        this.filling.delete(m.b);
      }
    } else if ("flush" in m) {
      this.sendRecording();
      this.worker?.postMessage({ flushed: m.flush });
    }
  }

  /** Sends collected microphone samples to the worker. */
  private sendRecording(): void {
    if (!this.recPos || !this.worker) return;
    const chunk = this.rec.slice(0, this.recPos);
    this.worker.postMessage({ rec: chunk }, [chunk.buffer]);
    this.recPos = 0;
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0]!;
    const left = out[0]!;
    const right = out[1] ?? left;
    const frames = left.length;
    const ptr = this.wasm.kwl_process(frames);
    const mix = new Float32Array(this.wasm.memory.buffer, ptr, frames * 2);
    for (let i = 0; i < frames; i++) {
      left[i] = mix[2 * i]!;
      right[i] = mix[2 * i + 1]!;
    }
    // Microphone (mono): level always, samples only during recording
    const input = inputs[0];
    if (input && input.length) {
      const n = input[0]!.length;
      for (let i = 0; i < n; i++) {
        let v = 0;
        for (const ch of input) v += ch[i]!;
        v /= input.length;
        const a = Math.abs(v);
        if (a > this.peakIn) this.peakIn = a;
        if (this.recording) {
          this.rec[this.recPos++] = v;
          if (this.recPos === REC_CHUNK) this.sendRecording();
        }
      }
    }
    if (++this.blocks >= STATUS_EVERY && this.worker) {
      this.blocks = 0;
      const sp = this.wasm.kwl_status();
      const s = new Float64Array(this.wasm.memory.buffer, sp, 4);
      this.worker.postMessage({ s: [s[0], s[1], s[2], s[3], this.peakIn] });
      this.peakIn = 0;
    }
    return true;
  }
}

registerProcessor("klangwelt", KlangweltProcessor);

// Module (own scope), loaded as an ES module.
export {};
