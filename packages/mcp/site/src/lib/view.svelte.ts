// Global UI preference: show the RAW model-facing view (the exact `llm_result`
// text and `llm_images` the agent received) instead of the human-rendered HTML.
// One switch in the header flips every output at once, so a curious human can
// see what the LLM actually saw. Persisted so a refresh keeps the choice.
const KEY = 'ix-mcp-raw-llm';

function load(): boolean {
  try {
    return localStorage.getItem(KEY) === '1';
  } catch {
    // Storage may be blocked; default to the human view.
    return false;
  }
}

class View {
  rawLLM = $state(load());

  toggle(): void {
    this.rawLLM = !this.rawLLM;
    try {
      localStorage.setItem(KEY, this.rawLLM ? '1' : '0');
    } catch {
      // Storage blocked: the choice still holds for this session.
    }
  }
}

export const view = new View();
