// Stands in for Testing Library's `act`, which testApi.tsx wraps every event it pushes in. In
// a browser React renders by itself; this only gives it a moment to, so that `await push(...)`
// returns once the screen shows the event.
export async function act(callback: () => unknown): Promise<void> {
  await callback();
  await new Promise((resolve) => setTimeout(resolve, 40));
}
