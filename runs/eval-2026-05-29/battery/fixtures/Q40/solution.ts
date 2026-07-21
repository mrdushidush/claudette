type Listener = (...args: unknown[]) => void;

export class EventEmitter {
  on(_event: string, _listener: Listener): void {}
  off(_event: string, _listener: Listener): void {}
  emit(_event: string, ..._args: unknown[]): void {}
  once(_event: string, _listener: Listener): void {}
}
