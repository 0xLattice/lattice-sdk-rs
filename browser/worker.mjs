import init, {run} from './pkg/browser_smoke.js';

try {
  if (!(globalThis instanceof DedicatedWorkerGlobalScope) || typeof window !== 'undefined') {
    throw Error('SDK checks must execute inside a dedicated worker');
  }
  await init();
  const checks = JSON.parse(await run(location.origin));
  postMessage({checks, worker: true});
} catch (error) {
  postMessage({error: String(error)});
}
