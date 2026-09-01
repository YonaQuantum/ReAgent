import React, { FormEvent, KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Activity, Bot, Check, ChevronDown, Download, FileCode2, FileText, FolderOpen,
  Loader2, Maximize2, Menu, MessageSquare, PanelLeftClose, PanelRightClose,
  Paperclip, Plus, Search, Send, Settings, Square, Trash2, UserRound, X,
  ZoomIn, ZoomOut,
} from "lucide-react";
import "./styles.css";

type Provider = "deepseek" | "openai-compatible" | "heuristic";
type InspectorTab = "activity" | "preview";
type RunStatus = "idle" | "running" | "done" | "error" | "cancelled";
type ConnectionStatus = "checking" | "connected" | "disconnected";

type Artifact = {
  kind: string;
  path: string;
  mime: string;
  engine?: string;
  width_cm?: number;
  height_cm?: number;
};

type AgentRunOutput = {
  run_id: string;
  final_message: string;
  artifacts: Artifact[];
  event_log_path?: string | null;
};

type Message = {
  id: string;
  role: "user" | "agent" | "artifact";
  content: string;
  artifactPath?: string;
};

type ActivityItem = {
  id: string;
  name: string;
  status: "running" | "success" | "error";
  startedAt: string;
  durationMs?: number;
  input?: unknown;
  output?: unknown;
  summary?: string;
};

type Conversation = {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messages: Message[];
  activities: ActivityItem[];
  artifacts: Artifact[];
  runId?: string;
};

type StreamEvent = {
  at?: string;
  kind?: {
    type?: string;
    id?: string;
    name?: string;
    args?: unknown;
    ok?: boolean;
    output?: unknown;
    artifact?: Artifact;
    content?: string;
  };
};

const STORAGE_KEY = "reagent.workspace.v2";
const ACTIVE_KEY = "reagent.active.v2";
const LEGACY_WELCOME = "你好，我是 ReAgent。告诉我你想完成什么，我会选择工具并把结果放到右侧工作区。";

function newConversation(): Conversation {
  const now = new Date().toISOString();
  return {
    id: createId(),
    title: "新对话",
    createdAt: now,
    updatedAt: now,
    messages: [],
    activities: [],
    artifacts: [],
  };
}

function App() {
  const initial = useMemo(loadConversations, []);
  const [conversations, setConversations] = useState<Conversation[]>(initial);
  const [activeId, setActiveId] = useState(() => {
    const saved = loadActiveId();
    return saved && initial.some((item) => item.id === saved) ? saved : initial[0].id;
  });
  const [prompt, setPrompt] = useState("");
  const [provider, setProvider] = useState<Provider>("deepseek");
  const [runStatuses, setRunStatuses] = useState<Record<string, RunStatus>>({});
  const [connectionStatus, setConnectionStatus] = useState<ConnectionStatus>("checking");
  const [agentName, setAgentName] = useState("ReAgent");
  const [search, setSearch] = useState("");
  const [sidebarOpen, setSidebarOpen] = useState(() => window.innerWidth >= 760);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("activity");
  const [selectedPath, setSelectedPath] = useState<string>();
  const [fullscreen, setFullscreen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [previewScale, setPreviewScale] = useState(1);
  const [isComposing, setIsComposing] = useState(false);
  const [showJump, setShowJump] = useState(false);
  const [attachments, setAttachments] = useState<{ path: string; name: string }[]>([]);
  const [uploading, setUploading] = useState(false);
  const controllersRef = useRef(new Map<string, AbortController>());
  const activeIdRef = useRef(activeId);
  const threadRef = useRef<HTMLElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const apiBase = useMemo(() => {
    const host = !location.hostname || location.hostname === "0.0.0.0" ? "127.0.0.1" : location.hostname;
    return `${location.protocol}//${host}:8787`;
  }, []);
  const active = conversations.find((item) => item.id === activeId) ?? conversations[0];
  const status = runStatuses[active.id] ?? "idle";
  const runningCount = Object.values(runStatuses).filter((item) => item === "running").length;
  const selectedArtifact = active.artifacts.find((item) => item.path === selectedPath) ?? active.artifacts[0];

  useEffect(() => {
    activeIdRef.current = activeId;
  }, [activeId]);

  useEffect(() => {
    let disposed = false;
    async function checkConnection() {
      try {
        const response = await fetch(`${apiBase}/health`, { signal: AbortSignal.timeout(4000) });
        const body = response.ok ? await response.json() as { ok?: boolean; name?: string } : null;
        if (!disposed) {
          setConnectionStatus(body?.ok === true ? "connected" : "disconnected");
          if (body?.name) setAgentName(body.name);
        }
      } catch {
        if (!disposed) setConnectionStatus("disconnected");
      }
    }
    void checkConnection();
    const timer = window.setInterval(checkConnection, 15000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [apiBase]);

  useEffect(() => {
    document.title = `${agentName} 工作区`;
  }, [agentName]);

  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(conversations));
  }, [conversations]);

  useEffect(() => {
    localStorage.setItem(ACTIVE_KEY, activeId);
  }, [activeId]);

  // Cross-tab sync: without this, each tab holds its own copy and whichever
  // tab writes last clobbers the others — switching back loses conversations.
  useEffect(() => {
    function onStorage(event: StorageEvent) {
      if (event.key === STORAGE_KEY && event.newValue) {
        try {
          const next = JSON.parse(event.newValue) as Conversation[];
          if (Array.isArray(next) && next.length) setConversations(next);
        } catch { /* ignore malformed */ }
      } else if (event.key === ACTIVE_KEY && event.newValue) {
        setActiveId(event.newValue);
      }
    }
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(Math.max(el.scrollHeight, 54), 180)}px`;
  }, [prompt]);

  useEffect(() => {
    const el = threadRef.current;
    if (!el || showJump) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [active.messages.length, status, showJump]);

  function updateConversation(id: string, updater: (conversation: Conversation) => Conversation) {
    setConversations((items) =>
      items.map((item) => item.id === id
        ? { ...updater(item), updatedAt: new Date().toISOString() }
        : item),
    );
  }

  function setConversationStatus(id: string, nextStatus: RunStatus) {
    setRunStatuses((current) => ({ ...current, [id]: nextStatus }));
  }

  function createConversation() {
    const conversation = newConversation();
    setConversations((items) => [conversation, ...items]);
    setActiveId(conversation.id);
    setPrompt("");
    setSelectedPath(undefined);
    setInspectorOpen(false);
  }

  function selectConversation(id: string) {
    setActiveId(id);
    setPrompt("");
    setSelectedPath(undefined);
    if (window.innerWidth < 760) setSidebarOpen(false);
  }

  function deleteConversation(id: string) {
    controllersRef.current.get(id)?.abort();
    controllersRef.current.delete(id);
    setRunStatuses((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
    const remaining = conversations.filter((item) => item.id !== id);
    if (remaining.length) {
      setConversations(remaining);
      if (activeId === id) setActiveId(remaining[0].id);
    } else {
      const fresh = newConversation();
      setConversations([fresh]);
      setActiveId(fresh.id);
    }
  }

  function openArtifact(artifact: Artifact) {
    setSelectedPath(artifact.path);
    setInspectorTab("preview");
    setInspectorOpen(true);
    setPreviewScale(1);
  }

  function stopRun(id = activeId) {
    controllersRef.current.get(id)?.abort();
    controllersRef.current.delete(id);
    setConversationStatus(id, "cancelled");
  }

  async function uploadFiles(files: FileList | File[]) {
    const list = Array.from(files);
    if (!list.length) return;
    setUploading(true);
    const next: { path: string; name: string }[] = [];
    for (const file of list) {
      try {
        const form = new FormData();
        form.append("file", file);
        const response = await fetch(`${apiBase}/api/upload`, { method: "POST", body: form });
        if (!response.ok) throw new Error(await response.text());
        const body = await response.json() as { path: string; name: string };
        next.push({ path: body.path, name: body.name });
      } catch (error) {
        console.error("上传失败", error);
      }
    }
    setAttachments((current) => [...current, ...next]);
    setUploading(false);
  }

  function removeAttachment(path: string) {
    setAttachments((current) => current.filter((item) => item.path !== path));
  }

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    const content = prompt.trim();
    if (!content || status === "running") return;

    const conversationId = activeId;
    const controller = new AbortController();
    controllersRef.current.set(conversationId, controller);
    const files = attachments.map((item) => item.path);
    setPrompt("");
    setAttachments([]);
    setConversationStatus(conversationId, "running");
    updateConversation(conversationId, (conversation) => ({
      ...conversation,
      title: conversation.title === "新对话" ? createTitle(content) : conversation.title,
      messages: [...conversation.messages, { id: createId(), role: "user", content }],
      activities: [],
    }));

    try {
      const response = await fetch(`${apiBase}/api/runs`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ prompt: content, provider, files }),
        signal: controller.signal,
      });
      if (!response.ok || !response.body) {
        throw new Error(await response.text() || `HTTP ${response.status}`);
      }

      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
        let boundary = buffer.indexOf("\n\n");
        while (boundary !== -1) {
          handleSseBlock(buffer.slice(0, boundary), conversationId);
          buffer = buffer.slice(boundary + 2);
          boundary = buffer.indexOf("\n\n");
        }
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      setConversationStatus(conversationId, "error");
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        messages: [...conversation.messages, { id: createId(), role: "agent", content: readableError(error) }],
      }));
    } finally {
      if (controllersRef.current.get(conversationId) === controller) {
        controllersRef.current.delete(conversationId);
      }
    }
  }

  function handleSseBlock(block: string, conversationId: string) {
    const { event, data } = parseSseBlock(block);
    if (!event) return;
    if (event === "done") {
      try {
        const output = JSON.parse(data) as AgentRunOutput;
        setConversationStatus(conversationId, "done");
        updateConversation(conversationId, (conversation) => {
          const paths = new Set(conversation.artifacts.map((artifact) => artifact.path));
          const artifacts = [...conversation.artifacts, ...output.artifacts.filter((artifact) => !paths.has(artifact.path))];
          const messages: Message[] = [
            ...conversation.messages,
            { id: createId(), role: "agent", content: output.final_message },
          ];
          const finalArtifacts = output.artifacts.some((artifact) => artifact.mime === "application/pdf")
            ? output.artifacts.filter((artifact) => artifact.mime === "application/pdf")
            : output.artifacts.filter((artifact) => artifact.kind !== "diagnostics");
          for (const artifact of finalArtifacts) {
            if (!conversation.messages.some((message) => message.artifactPath === artifact.path)) {
              messages.push({ id: createId(), role: "artifact", content: artifactTitle(artifact), artifactPath: artifact.path });
            }
          }
          return { ...conversation, runId: output.run_id, messages, artifacts };
        });
        if (output.artifacts.length && activeIdRef.current === conversationId) {
          openArtifact(
            output.artifacts.find((item) => item.kind === "preview")
              ?? output.artifacts.find((item) => item.mime.startsWith("image/"))
              ?? output.artifacts.find((item) => item.mime === "application/pdf")
              ?? output.artifacts[0],
          );
        }
      } catch {
        setConversationStatus(conversationId, "error");
      }
      return;
    }
    if (event === "error") {
      setConversationStatus(conversationId, "error");
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        messages: [...conversation.messages, { id: createId(), role: "agent", content: data || "运行失败" }],
      }));
      return;
    }

    const payload = tryParse(data);
    const kind = payload?.kind;
    if (!kind) return;
    if (event === "tool_call" && kind.id && kind.name) {
      if (activeIdRef.current === conversationId) {
        setInspectorOpen(true);
        setInspectorTab("activity");
      }
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        activities: [...conversation.activities, {
          id: kind.id!,
          name: kind.name!,
          status: "running",
          startedAt: payload.at ?? new Date().toISOString(),
          input: kind.args,
        }],
      }));
    }
    if (event === "tool_result" && kind.id) {
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        activities: conversation.activities.map((item) => item.id === kind.id ? {
          ...item,
          status: kind.ok ? "success" : "error",
          output: kind.output,
          summary: summarizeToolResult(item.name, kind.output),
          durationMs: Math.max(0, Date.now() - new Date(item.startedAt).getTime()),
        } : item),
      }));
    }
    if (event === "artifact_created" && kind.artifact) {
      const artifact = kind.artifact;
      updateConversation(conversationId, (conversation) => conversation.artifacts.some((item) => item.path === artifact.path)
        ? conversation
        : { ...conversation, artifacts: [...conversation.artifacts, artifact] });
      if (activeIdRef.current === conversationId) openArtifact(artifact);
    }
  }

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing || isComposing) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  }

  const filtered = conversations.filter((item) => item.title.toLowerCase().includes(search.toLowerCase()));
  const grouped = groupConversations(filtered);
  const shellClass = [
    "workspace",
    sidebarOpen ? "withSidebar" : "",
    inspectorOpen ? "withInspector" : "",
  ].filter(Boolean).join(" ");

  return (
    <main className={shellClass}>
      <aside className={`sidebar ${sidebarOpen ? "open" : ""}`}>
        <div className="sidebarTop">
          <div className="brand"><span className="brandMark">{agentName.charAt(0)}</span><strong>{agentName}</strong></div>
          <button className="iconButton desktopOnly" onClick={() => setSidebarOpen(false)} aria-label="收起侧栏"><PanelLeftClose size={17} /></button>
        </div>
        <button className="newChat" onClick={createConversation}><Plus size={17} />新建对话</button>
        <label className="searchBox"><Search size={15} /><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索对话" /></label>
        <nav className="conversationList" aria-label="历史对话">
          {Object.entries(grouped).map(([group, items]) => items.length > 0 && (
            <section className="conversationGroup" key={group}>
              <h2>{group}</h2>
              {items.map((item) => (
                <div className={`conversationItem ${item.id === activeId ? "active" : ""}`} key={item.id}>
                  <button className="conversationMain" onClick={() => selectConversation(item.id)}><MessageSquare size={15} /><span>{item.title}</span></button>
                  <ConversationState status={runStatuses[item.id] ?? "idle"} />
                  <button className="conversationDelete" onClick={() => deleteConversation(item.id)} aria-label={`删除 ${item.title}`}><Trash2 size={14} /></button>
                </div>
              ))}
            </section>
          ))}
        </nav>
        <button className="settingsButton" onClick={() => setSettingsOpen(true)}><Settings size={16} />设置</button>
      </aside>

      {(sidebarOpen || inspectorOpen) && <button className="drawerBackdrop" onClick={() => { setSidebarOpen(false); setInspectorOpen(false); }} aria-label="关闭面板" />}

      <section className="chatPane">
        <header className="workspaceHeader">
          <div className="headerTitle">
            {!sidebarOpen && <button className="iconButton" onClick={() => setSidebarOpen(true)} aria-label="打开侧栏"><Menu size={18} /></button>}
            <div><h1>{active.title}</h1><span>通用执行 Agent</span></div>
          </div>
          <div className="headerActions">
            <label className="modelSelect"><Bot size={15} /><select value={provider} onChange={(event) => setProvider(event.target.value as Provider)} disabled={status === "running"}><option value="deepseek">DeepSeek</option><option value="openai-compatible">OpenAI Compatible</option><option value="heuristic">本地模式</option></select><ChevronDown size={14} /></label>
            <RunBadge status={status} runningCount={runningCount} />
            {!inspectorOpen && <button className="iconButton inspectorTrigger" onClick={() => setInspectorOpen(true)} aria-label="打开检查器"><Activity size={18} /></button>}
          </div>
        </header>

        <section className="thread" ref={threadRef} aria-live="polite" onScroll={(event) => {
          const el = event.currentTarget;
          setShowJump(el.scrollHeight - el.scrollTop - el.clientHeight > 140);
        }}>
          <div className={`threadInner ${active.messages.length === 0 ? "empty" : ""}`}>
            {active.messages.length === 0 && status === "idle"
              ? <div className="emptyConversation">我们应该构建什么？</div>
              : active.messages.map((message) => message.role === "artifact"
                ? <ArtifactCard key={message.id} message={message} artifacts={active.artifacts} onOpen={openArtifact} />
                : <ChatMessage key={message.id} message={message} agentName={agentName} />)}
            {status === "running" && <div className="workingLine"><Loader2 className="spin" size={15} /><span>正在执行</span><i /><i /><i /></div>}
            {status === "cancelled" && <div className="runNotice">本次运行已停止</div>}
          </div>
          {showJump && <button className="jumpLatest" onClick={() => threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: "smooth" })}>回到最新消息<ChevronDown size={14} /></button>}
        </section>

        <div className="composerDock">
          <form className="composer" onSubmit={submit} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); void uploadFiles(event.dataTransfer.files); }}>
            {attachments.length > 0 && (
              <div className="attachmentRow">
                {attachments.map((item) => (
                  <span className="attachmentChip" key={item.path}>
                    <FileText size={13} />
                    <span>{item.name}</span>
                    <button type="button" onClick={() => removeAttachment(item.path)} aria-label={`移除 ${item.name}`}><X size={12} /></button>
                  </span>
                ))}
              </div>
            )}
            <textarea ref={textareaRef} value={prompt} onChange={(event) => setPrompt(event.target.value)} onKeyDown={onComposerKeyDown} onCompositionStart={() => setIsComposing(true)} onCompositionEnd={() => setIsComposing(false)} placeholder={`给 ${agentName} 一个任务`} aria-label="任务内容" rows={1} />
            <div className="composerBar">
              <input ref={fileInputRef} type="file" multiple hidden onChange={(event) => { if (event.target.files) void uploadFiles(event.target.files); event.target.value = ""; }} />
              <button type="button" className="attachButton" onClick={() => fileInputRef.current?.click()} disabled={status === "running"} aria-label="附加文件" title="附加文件"><Paperclip size={15} />{uploading && <Loader2 className="spin" size={13} />}</button>
              <span>Enter 发送 · Shift + Enter 换行</span>
              {status === "running"
                ? <button type="button" className="sendButton stop" onClick={() => stopRun()} aria-label="停止运行"><Square size={14} fill="currentColor" /></button>
                : <button type="submit" className="sendButton" disabled={!prompt.trim()} aria-label="发送"><Send size={17} /></button>}
            </div>
          </form>
          <p>{agentName} 可能会犯错，请检查重要结果。</p>
        </div>
      </section>

      <aside className={`inspector ${inspectorOpen ? "open" : ""}`}>
        <header className="inspectorHeader">
          <div className="tabs" role="tablist">
            <button className={inspectorTab === "activity" ? "active" : ""} onClick={() => setInspectorTab("activity")}><Activity size={15} />活动{active.activities.length > 0 && <b>{active.activities.length}</b>}</button>
            <button className={inspectorTab === "preview" ? "active" : ""} onClick={() => setInspectorTab("preview")}><FolderOpen size={15} />预览{active.artifacts.length > 0 && <b>{active.artifacts.length}</b>}</button>
          </div>
          <button className="iconButton" onClick={() => setInspectorOpen(false)} aria-label="关闭检查器"><PanelRightClose size={18} /></button>
        </header>
        {inspectorTab === "activity"
          ? <ActivityPanel activities={active.activities} status={status} runId={active.runId} />
          : <PreviewPanel artifact={selectedArtifact} artifacts={active.artifacts} apiBase={apiBase} scale={previewScale} onScale={setPreviewScale} onSelect={openArtifact} onFullscreen={() => setFullscreen(true)} />}
      </aside>

      {fullscreen && selectedArtifact && <div className="modalBackdrop" role="dialog" aria-modal="true" aria-label="产物全屏预览"><div className="previewModal"><button className="modalClose iconButton" onClick={() => setFullscreen(false)} aria-label="关闭全屏预览"><X size={20} /></button><ArtifactViewer artifact={selectedArtifact} apiBase={apiBase} scale={1} /></div></div>}
      {settingsOpen && <div className="modalBackdrop" role="dialog" aria-modal="true" aria-label="设置">
        <section className="settingsModal">
          <header><div><h2>设置</h2><p>工作区与模型连接</p></div><button className="iconButton" onClick={() => setSettingsOpen(false)} aria-label="关闭设置"><X size={18} /></button></header>
          <label><span>默认模型</span><select value={provider} onChange={(event) => setProvider(event.target.value as Provider)}><option value="deepseek">DeepSeek</option><option value="openai-compatible">OpenAI Compatible</option><option value="heuristic">本地模式</option></select></label>
          <div className="connectionRow"><span>Kernel API</span><code>{apiBase}</code><ConnectionBadge status={connectionStatus} /></div>
        </section>
      </div>}
    </main>
  );
}

function ChatMessage({ message, agentName }: { message: Message; agentName: string }) {
  return <article className={`chatMessage ${message.role}`}>
    <div className="messageAvatar">{message.role === "user" ? <UserRound size={16} /> : <span>{agentName.charAt(0)}</span>}</div>
    <div className="messageBody"><div className="messageAuthor">{message.role === "user" ? "你" : agentName}</div><MessageContent content={message.content} /></div>
  </article>;
}

function MessageContent({ content }: { content: string }) {
  return <div className="messageText">{content.split("\n").map((line, index) => {
    const value = line.replace(/^#+\s*/, "");
    const isList = /^[-*]\s+/.test(value);
    return <div className={isList ? "markdownList" : value ? undefined : "markdownGap"} key={`${index}-${line}`}>
      {isList && <span className="listMarker">•</span>}<span className="lineContent">{formatInline(value.replace(/^[-*]\s+/, ""))}</span>
    </div>;
  })}</div>;
}

function formatInline(line: string): React.ReactNode[] {
  return line.split(/(`[^`]+`|\*\*[^*]+\*\*)/g).filter(Boolean).map((part, index) => {
    if (part.startsWith("`") && part.endsWith("`")) return <code key={index}>{part.slice(1, -1)}</code>;
    if (part.startsWith("**") && part.endsWith("**")) return <strong key={index}>{part.slice(2, -2)}</strong>;
    return <React.Fragment key={index}>{part}</React.Fragment>;
  });
}

function ArtifactCard({ message, artifacts, onOpen }: { message: Message; artifacts: Artifact[]; onOpen: (artifact: Artifact) => void }) {
  const artifact = artifacts.find((item) => item.path === message.artifactPath);
  if (!artifact) return null;
  return <button className="artifactCard" onClick={() => onOpen(artifact)}>
    <span className="artifactFileIcon">{artifact.mime === "text/html" ? <FileCode2 size={19} /> : <FileText size={19} />}</span>
    <span><strong>{message.content}</strong><small>{artifact.mime} · {fileName(artifact.path)}</small></span>
    <span className="artifactOpen">打开预览</span>
  </button>;
}

function ConversationState({ status }: { status: RunStatus }) {
  if (status === "running") return <span className="conversationState running" title="正在运行"><Loader2 size={13} className="spin" /></span>;
  if (status === "done") return <span className="conversationState done" title="已完成"><Check size={13} /></span>;
  if (status === "error") return <span className="conversationState error" title="运行失败"><X size={13} /></span>;
  return null;
}

function RunBadge({ status, runningCount }: { status: RunStatus; runningCount: number }) {
  const isRunning = status === "running" || runningCount > 0;
  const label = status === "running"
    ? runningCount > 1 ? `${runningCount} 个任务运行中` : "运行中"
    : runningCount > 0 ? `${runningCount} 个后台任务`
    : status === "error" ? "失败"
    : status === "cancelled" ? "已停止"
    : "就绪";
  return <span className={`runBadge ${isRunning ? "running" : status}`}><i />{label}</span>;
}

function ConnectionBadge({ status }: { status: ConnectionStatus }) {
  const label = status === "connected" ? "已连接" : status === "checking" ? "检测中" : "未连接";
  return <b className={`connectionBadge ${status}`}>{status === "checking" ? <Loader2 size={12} className="spin" /> : <i />}{label}</b>;
}

function ActivityPanel({ activities, status, runId }: { activities: ActivityItem[]; status: RunStatus; runId?: string }) {
  if (!activities.length) return <EmptyInspector icon={<Activity size={22} />} title="还没有执行活动" description="Agent 调用工具后，真实过程会显示在这里。" />;
  return <div className="activityPanel">
    <div className="runSummary"><span>本次运行</span><strong>{status === "running" ? "正在执行" : status === "error" ? "执行失败" : "执行完成"}</strong>{runId && <code>{runId.slice(0, 8)}</code>}</div>
    <ol className="timeline">{activities.map((item) => <li key={item.id} className={item.status}>
      <span className="timelineIcon">{item.status === "running" ? <Loader2 className="spin" size={14} /> : item.status === "success" ? <Check size={14} /> : <X size={14} />}</span>
      <div className="activityBody"><div><strong>{displayToolName(item.name)}</strong><time>{item.durationMs === undefined ? "进行中" : formatDuration(item.durationMs)}</time></div><code>{item.name}</code>{item.summary && <p>{item.summary}</p>}
        {(item.input !== undefined || item.output !== undefined) && <details><summary>查看详情</summary><pre>{prettyJson(item.output ?? item.input)}</pre></details>}
      </div>
    </li>)}</ol>
  </div>;
}

function PreviewPanel({ artifact, artifacts, apiBase, scale, onScale, onSelect, onFullscreen }: { artifact?: Artifact; artifacts: Artifact[]; apiBase: string; scale: number; onScale: (scale: number) => void; onSelect: (artifact: Artifact) => void; onFullscreen: () => void }) {
  if (!artifact) return <EmptyInspector icon={<FolderOpen size={22} />} title="还没有产物" description="生成的 HTML、PDF 和图片会在这里预览。" />;
  const url = artifactUrl(apiBase, artifact.path);
  return <div className="previewPanel">
    <div className="artifactHeader"><div><strong>{artifactTitle(artifact)}</strong><span>{artifact.mime}{artifact.width_cm ? ` · ${artifact.width_cm} × ${artifact.height_cm} cm` : ""}</span></div><a className="iconButton" href={url} download title="下载产物" aria-label="下载产物"><Download size={17} /></a></div>
    {artifacts.length > 1 && <div className="artifactSwitcher">{artifacts.map((item) => <button key={item.path} className={item.path === artifact.path ? "active" : ""} onClick={() => onSelect(item)}>{item.kind.toUpperCase()}</button>)}</div>}
    <div className="previewToolbar"><button onClick={() => onScale(Math.max(.5, scale - .1))} aria-label="缩小"><ZoomOut size={16} /></button><span>{Math.round(scale * 100)}%</span><button onClick={() => onScale(Math.min(1.8, scale + .1))} aria-label="放大"><ZoomIn size={16} /></button><button onClick={onFullscreen} aria-label="全屏预览"><Maximize2 size={16} /></button></div>
    <ArtifactViewer artifact={artifact} apiBase={apiBase} scale={scale} />
  </div>;
}

function ArtifactViewer({ artifact, apiBase, scale }: { artifact: Artifact; apiBase: string; scale: number }) {
  const url = artifactUrl(apiBase, artifact.path);
  if (artifact.mime.startsWith("image/")) return <div className="viewer imageViewer"><img src={url} alt={artifactTitle(artifact)} style={{ transform: `scale(${scale})` }} /></div>;
  if (artifact.mime === "application/pdf") return <div className="viewer"><iframe title="PDF 产物预览" src={`${url}#toolbar=0&navpanes=0&view=FitH`} style={{ width: `${100 / scale}%`, height: `${100 / scale}%`, transform: `scale(${scale})`, transformOrigin: "top left" }} /></div>;
  if (artifact.mime === "text/html") return <div className="viewer"><iframe title="HTML 产物预览" sandbox="" src={url} style={{ width: `${100 / scale}%`, height: `${100 / scale}%`, transform: `scale(${scale})`, transformOrigin: "top left" }} /></div>;
  return <div className="viewer fallbackViewer"><FileText size={30} /><span>此格式可下载后查看</span><a href={url} download>下载 {fileName(artifact.path)}</a></div>;
}

function EmptyInspector({ icon, title, description }: { icon: React.ReactNode; title: string; description: string }) {
  return <div className="emptyInspector"><span>{icon}</span><strong>{title}</strong><p>{description}</p></div>;
}

function parseSseBlock(block: string): { event: string; data: string } {
  let event = "";
  const dataLines: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) dataLines.push(line.slice(5).replace(/^\s/, ""));
  }
  return { event, data: dataLines.join("\n") };
}

function tryParse(data: string): StreamEvent | null { try { return JSON.parse(data) as StreamEvent; } catch { return null; } }
function prettyJson(value: unknown) { try { return JSON.stringify(value, null, 2); } catch { return String(value); } }
function fileName(path: string) { return path.split("/").pop() || path; }
function artifactTitle(artifact: Artifact) { const names: Record<string, string> = { pdf: "PDF 文件", html: "HTML 源文件", preview: "设计预览", png: "整页截图", trajectory: "运行轨迹" }; return names[artifact.kind] ?? fileName(artifact.path); }
function displayToolName(name: string) { const names: Record<string, string> = { render_banner: "生成设计", lint_banner: "检查排版", export_banner: "导出文件", render_page: "渲染网页", lint_page: "检查网页", export_page: "导出网页", write_file: "写入文件", read_file: "读取文件", list_files: "查看文件" }; return names[name] ?? name; }
function formatDuration(ms: number) { return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`; }
function readableError(error: unknown) { if (!(error instanceof Error)) return "运行失败，请稍后重试。"; try { const parsed = JSON.parse(error.message) as { error?: string }; return parsed.error ?? error.message; } catch { return error.message; } }
function createTitle(content: string) { return content.replace(/\s+/g, " ").slice(0, 24) || "新对话"; }
function artifactUrl(apiBase: string, path: string) { const normalized = path.replaceAll("\\", "/"); const marker = "/artifacts/"; const index = normalized.indexOf(marker); return index === -1 ? "#" : `${apiBase}${marker}${normalized.slice(index + marker.length)}`; }
function summarizeToolResult(name: string, output: unknown): string { if (!output || typeof output !== "object") return "执行完成"; const o = output as Record<string, unknown>; if (name === "lint_banner" || name === "lint_page") return o.passed === true ? "排版检查通过" : String(o.summary ?? "检查未通过"); if (name === "export_banner") return o.exported === true ? "PDF 与预览文件已导出" : String(o.error ?? "导出失败"); if (name === "export_page") return o.exported === true ? "网页已导出" : String(o.error ?? "导出失败"); if (typeof o.summary === "string") return o.summary; if (typeof o.path === "string") return `已写入 ${fileName(o.path)}`; return "执行完成"; }
function groupConversations(items: Conversation[]) { const result: Record<string, Conversation[]> = { 今天: [], 昨天: [], "过去 7 天": [], 更早: [] }; const now = new Date(); for (const item of items) { const days = Math.floor((now.getTime() - new Date(item.updatedAt).getTime()) / 86400000); result[days <= 0 ? "今天" : days === 1 ? "昨天" : days <= 7 ? "过去 7 天" : "更早"].push(item); } return result; }
function loadConversations(): Conversation[] {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as Conversation[];
    if (Array.isArray(saved) && saved.length) {
      return saved.map((conversation) => ({
        ...conversation,
        messages: conversation.messages.filter((message) => message.content !== LEGACY_WELCOME),
      }));
    }
  } catch { /* start fresh */ }
  return [newConversation()];
}
function loadActiveId(): string | null { try { return localStorage.getItem(ACTIVE_KEY); } catch { return null; } }
function createId() { if (globalThis.crypto && typeof globalThis.crypto.randomUUID === "function") return globalThis.crypto.randomUUID(); return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`; }

createRoot(document.getElementById("root")!).render(<App />);
