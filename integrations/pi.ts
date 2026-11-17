import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

type AgentState = "idle" | "working" | "waiting" | "error" | "stopped";

interface RuntimeState {
  active: boolean;
  prompts: number;
  error: string;
  prompt: string;
  children: Map<string, string[]>;
}

function publish(pi: ExtensionAPI, ctx: ExtensionContext, state: AgentState, runtime: RuntimeState, model?: string): Promise<void> {
  const binary = join(homedir(), "Documents", "tmux-agent-sidebar", "bin", "tmux-agent-sidebar.exe");
  const payload = {
    agent_state: state,
    session_id: ctx.sessionManager.getSessionId(),
    transcript_path: ctx.sessionManager.getSessionFile(),
    conversation_title: pi.getSessionName(),
    model: model ?? ctx.model?.name ?? ctx.model?.id,
    cwd: ctx.cwd,
    prompt: runtime.prompt,
    reason: runtime.error,
    task_count: runtime.children.size,
    subagents: [...runtime.children.values()].flat().map((name) => ({ name, status: "running" })),
  };
  return new Promise((resolve, reject) => {
    const child = spawn(binary, ["report", "pi"], { windowsHide: true, stdio: ["pipe", "ignore", "pipe"] });
    let error = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => { error += chunk; });
    child.on("error", reject);
    child.stdin.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`Sidebar report failed (${code}): ${error}`));
    });
    child.stdin.end(JSON.stringify(payload));
  });
}

function currentState(runtime: RuntimeState): AgentState {
  if (runtime.prompts > 0) return "waiting";
  if (runtime.error) return "error";
  return runtime.active ? "working" : "idle";
}

export default function sidebar(pi: ExtensionAPI): void {
  if (!process.env.TMUX_PANE) return;
  const runtime: RuntimeState = { active: false, prompts: 0, error: "", prompt: "", children: new Map() };
  let enabled = false;
  let context: ExtensionContext | undefined;

  function reportBackground(): void {
    if (!enabled || !context) return;
    const ctx = context;
    void publish(pi, ctx, currentState(runtime), runtime).catch((error: unknown) => {
      ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
    });
  }

  pi.events.on("subagent:async-started", (data: unknown) => {
    if (!enabled || !context) return;
    if (typeof data !== "object" || data === null) throw new Error("Invalid Pi subagent start event");
    const event = data as Record<string, unknown>;
    if (event.sessionId !== context.sessionManager.getSessionId() || event.mode === "workflow") return;
    if (typeof event.id !== "string") throw new Error("Pi subagent event has no run ID");
    const names = Array.isArray(event.agents) ? event.agents.filter((name): name is string => typeof name === "string")
      : typeof event.agent === "string" ? [event.agent] : [];
    runtime.children.set(event.id, names);
    reportBackground();
  });
  pi.events.on("subagent:async-complete", (data: unknown) => {
    if (typeof data !== "object" || data === null) throw new Error("Invalid Pi subagent completion event");
    const event = data as Record<string, unknown>;
    if (typeof event.id !== "string" || !runtime.children.delete(event.id)) return;
    reportBackground();
  });

  pi.on("session_start", async (_event, ctx) => {
    enabled = ctx.mode === "tui";
    if (!enabled) return;
    context = ctx;
    runtime.error = "";
    runtime.prompt = "";
    runtime.prompts = 0;
    runtime.children.clear();
    runtime.active = !ctx.isIdle();
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("before_agent_start", async (event, ctx) => {
    if (!enabled) return;
    if (event.messageType === "user") runtime.prompt = event.prompt;
    runtime.error = "";
    runtime.active = true;
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("agent_start", async (_event, ctx) => {
    if (!enabled) return;
    runtime.active = true;
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("agent_settled", async (_event, ctx) => {
    if (!enabled || !ctx.isIdle()) return;
    runtime.active = false;
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("ui_prompt_start", async (_event, ctx) => {
    if (!enabled) return;
    runtime.prompts += 1;
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("ui_prompt_end", async (_event, ctx) => {
    if (!enabled) return;
    runtime.prompts -= 1;
    if (runtime.prompts < 0) throw new Error("Sidebar received an unmatched Pi prompt end");
    await publish(pi, ctx, currentState(runtime), runtime);
  });
  pi.on("message_end", async (event, ctx) => {
    if (!enabled || event.message.role !== "assistant" || event.message.stopReason !== "error") return;
    runtime.error = event.message.errorMessage ?? "Model request failed";
    await publish(pi, ctx, "error", runtime);
  });
  pi.on("model_select", async (event, ctx) => {
    if (!enabled) return;
    await publish(pi, ctx, currentState(runtime), runtime, event.model.name);
  });
  pi.on("session_shutdown", async (_event, ctx) => {
    if (!enabled) return;
    await publish(pi, ctx, "stopped", runtime);
  });
}
