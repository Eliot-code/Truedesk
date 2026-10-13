import { useEffect, useRef, useState } from "react";

export interface ChatEntry {
  from: "me" | "peer";
  text: string;
  at: number;
}

export interface ChatPanelProps {
  open: boolean;
  messages: ChatEntry[];
  onSend: (text: string) => void;
  onClose: () => void;
}

/** Stays mounted for the whole session so messages are never missed; only the panel is hidden while closed. */
export default function ChatPanel({ open, messages, onSend, onClose }: ChatPanelProps) {
  const [draft, setDraft] = useState("");
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the newest message in view as the transcript grows or the panel opens.
  useEffect(() => {
    if (open && listRef.current) listRef.current.scrollTop = listRef.current.scrollHeight;
  }, [open, messages]);

  const submit = () => {
    const text = draft.trim();
    if (!text) return;
    onSend(text);
    setDraft("");
  };

  return (
    <section className="chat-panel" aria-label="Chat" hidden={!open}>
      <div className="chat-panel__header"><span className="eyebrow">CHAT</span><button className="icon-button chat-panel__close" type="button" onClick={onClose} aria-label="Close chat">×</button></div>
      <div className="chat-panel__list" ref={listRef}>
        {messages.length === 0 && <p className="chat-panel__empty">No messages yet.</p>}
        {messages.map((message, index) => (
          <div className={`chat-panel__message chat-panel__message--${message.from}`} key={`${message.at}-${index}`}>
            <span className="chat-panel__bubble">{message.text}</span>
          </div>
        ))}
      </div>
      <div className="chat-panel__compose">
        <input
          className="chat-panel__input"
          aria-label="Message"
          placeholder="Type a message…"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); submit(); } }}
        />
        <button className="primary-button" type="button" onClick={submit}>Send</button>
      </div>
    </section>
  );
}
