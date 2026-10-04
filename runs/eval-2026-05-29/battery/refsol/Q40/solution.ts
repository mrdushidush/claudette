type Listener = (...args: unknown[]) => void;

export class EventEmitter {
  private listeners: Map<string, Listener[]> = new Map();

  on(event: string, listener: Listener): void {
    const arr = this.listeners.get(event) ?? [];
    arr.push(listener);
    this.listeners.set(event, arr);
  }

  off(event: string, listener: Listener): void {
    const arr = this.listeners.get(event);
    if (!arr) return;
    this.listeners.set(
      event,
      arr.filter((l) => l !== listener),
    );
  }

  emit(event: string, ...args: unknown[]): void {
    const arr = this.listeners.get(event);
    if (!arr) return;
    for (const listener of [...arr]) {
      listener(...args);
    }
  }

  once(event: string, listener: Listener): void {
    const wrapper: Listener = (...args) => {
      this.off(event, wrapper);
      listener(...args);
    };
    this.on(event, wrapper);
  }
}
