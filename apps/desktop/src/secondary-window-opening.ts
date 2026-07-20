export class SecondaryWindowOpeningCoordinator<Role> {
  private readonly openings = new Map<Role, { token: symbol; promise: Promise<void> }>();

  run(role: Role, task: (token: symbol) => void | Promise<void>): Promise<void> {
    const existing = this.openings.get(role);
    if (existing) return existing.promise;

    const token = Symbol("secondary-window-opening");
    let started: Promise<void>;
    try {
      started = Promise.resolve(task(token));
    } catch (error) {
      started = Promise.reject(error);
    }
    const tracked = started.finally(() => {
      if (this.openings.get(role)?.token === token) this.openings.delete(role);
    });
    this.openings.set(role, { token, promise: tracked });
    return tracked;
  }

  clear(role: Role, token: symbol): void {
    if (this.openings.get(role)?.token === token) this.openings.delete(role);
  }
}
