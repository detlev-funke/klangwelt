// Minimal reactivity: signals + effects, batched via microtask.

interface Effect {
  run(): void;
  deps: Set<Source>;
  disposed: boolean;
}

interface Source {
  unsubscribe(e: Effect): void;
}

let current: Effect | null = null;
const queue = new Set<Effect>();
let scheduled = false;

function schedule(e: Effect): void {
  if (e.disposed) return;
  queue.add(e);
  if (scheduled) return;
  scheduled = true;
  queueMicrotask(() => {
    scheduled = false;
    const list = [...queue];
    queue.clear();
    for (const x of list) x.run();
  });
}

export class Signal<T> implements Source {
  #value: T;
  #subs = new Set<Effect>();

  constructor(value: T) {
    this.#value = value;
  }

  get value(): T {
    if (current) {
      this.#subs.add(current);
      current.deps.add(this);
    }
    return this.#value;
  }

  set value(v: T) {
    if (Object.is(v, this.#value)) return;
    this.#value = v;
    for (const e of this.#subs) schedule(e);
  }

  /** Read without a dependency. */
  peek(): T {
    return this.#value;
  }

  update(fn: (v: T) => T): void {
    this.value = fn(this.#value);
  }

  unsubscribe(e: Effect): void {
    this.#subs.delete(e);
  }
}

export function signal<T>(value: T): Signal<T> {
  return new Signal(value);
}

/** Runs `fn` immediately and again as soon as a signal it read changes. */
export function effect(fn: () => void): () => void {
  const e: Effect = {
    deps: new Set(),
    disposed: false,
    run() {
      if (e.disposed) return;
      for (const d of e.deps) d.unsubscribe(e);
      e.deps.clear();
      const prev = current;
      current = e;
      try {
        fn();
      } finally {
        current = prev;
      }
    },
  };
  e.run();
  return () => {
    e.disposed = true;
    for (const d of e.deps) d.unsubscribe(e);
    e.deps.clear();
  };
}
