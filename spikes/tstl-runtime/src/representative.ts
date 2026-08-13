class HookCounter {
  private total = 0;
  constructor(private readonly prefix: string) {}
  hook(values: number[]): string {
    const normalized = values.filter(value => value >= 2).map(value => value * 3);
    const sum = normalized.reduce((acc, value) => acc + value, 0);
    const closeOver = (suffix: string) => `${this.prefix.toUpperCase()}:${sum}:${suffix.toLowerCase()}`;
    this.total += sum;
    return closeOver(normalized.join("-"));
  }
  count(): number { return this.total; }
}
const handler = new HookCounter("hook");
const first = handler.hook([1, 2, 4]);
const second = handler.hook([3]);
declare let SPIKE_RESULT: string;
SPIKE_RESULT = `${first}|${second}|${handler.count()}`;
