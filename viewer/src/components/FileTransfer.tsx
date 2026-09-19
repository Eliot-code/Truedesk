import { useEffect, useState, type DragEvent } from "react";
import * as api from "../lib/api";

interface Transfer {
  id: number;
  name: string;
  size: number;
  direction: "send" | "receive";
  transferred: number;
  total: number;
  status: "transferring" | "done" | "error";
  error?: string;
}

interface PendingOffer extends api.FileOffer { responding?: boolean }

export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  do { value /= 1024; unit += 1; } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function subscription<T>(subscribe: (handler: (payload: T) => void) => Promise<() => void>, handler: (payload: T) => void, active: () => boolean, add: (stop: () => void) => void) {
  void subscribe(handler).then((stop) => { if (active()) add(stop); else stop(); });
}

export default function FileTransfer() {
  const [path, setPath] = useState("");
  const [message, setMessage] = useState("");
  const [offers, setOffers] = useState<PendingOffer[]>([]);
  const [transfers, setTransfers] = useState<Transfer[]>([]);

  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    const add = (stop: () => void) => stops.push(stop);

    subscription(api.onFileOffer, (offer) => {
      setOffers((current) => [...current.filter(({ id }) => id !== offer.id), offer]);
      setTransfers((current) => current.some(({ id }) => id === offer.id) ? current : [...current, {
        id: offer.id, name: offer.name, size: offer.size, direction: "receive", transferred: 0,
        total: offer.size, status: "transferring",
      }]);
    }, () => active, add);
    subscription(api.onFileProgress, (progress) => setTransfers((current) => {
      const existing = current.find(({ id }) => id === progress.id);
      if (!existing) return [...current, { id: progress.id, name: "File transfer", size: progress.total, direction: progress.direction === "receive" ? "receive" : "send", transferred: progress.transferred, total: progress.total, status: "transferring" }];
      return current.map((transfer) => transfer.id === progress.id ? { ...transfer, transferred: progress.transferred, total: progress.total, size: progress.total, direction: progress.direction === "receive" ? "receive" : "send" } : transfer);
    }), () => active, add);
    subscription(api.onFileDone, ({ id }) => setTransfers((current) => current.map((transfer) => transfer.id === id ? { ...transfer, transferred: transfer.total, status: "done" } : transfer)), () => active, add);
    subscription(api.onFileError, ({ id, message: error }) => setTransfers((current) => current.map((transfer) => transfer.id === id ? { ...transfer, status: "error", error } : transfer)), () => active, add);
    return () => { active = false; stops.forEach((stop) => stop()); };
  }, []);

  const send = async () => {
    const selectedPath = path.trim();
    if (!selectedPath) return;
    setMessage("");
    try {
      const id = await api.sendFile(selectedPath);
      setTransfers((current) => [...current, { id, name: selectedPath.split(/[\\/]/).pop() || selectedPath, size: 0, direction: "send", transferred: 0, total: 0, status: "transferring" }]);
      setPath("");
    } catch (error) { setMessage(error instanceof Error ? error.message : "Could not send file"); }
  };

  const dropPath = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    const file = event.dataTransfer.files[0] as (File & { path?: string }) | undefined;
    if (file?.path) setPath(file.path);
    else setMessage("Drop a file path or enter it below");
  };

  const answer = async (offer: PendingOffer, accept: boolean) => {
    setOffers((current) => current.map((item) => item.id === offer.id ? { ...item, responding: true } : item));
    try { await api.respondFile(offer.id, accept); setOffers((current) => current.filter(({ id }) => id !== offer.id)); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Could not respond to file offer"); setOffers((current) => current.map((item) => item.id === offer.id ? { ...item, responding: false } : item)); }
  };

  return <section className="file-transfer" aria-label="File transfer">
    <div className="file-transfer__send" onDragOver={(event) => event.preventDefault()} onDrop={dropPath}>
      <span className="eyebrow">FILE TRANSFER</span>
      <label htmlFor="file-path">Send file</label>
      <div className="file-transfer__control"><input id="file-path" value={path} onChange={(event) => setPath(event.target.value)} placeholder="Enter a file path" /><button className="primary-button" type="button" onClick={() => void send()} disabled={!path.trim()}>Send</button></div>
      <small>Drop a file here, or use a path from this device.</small>
      {message && <p className="message" role="alert">{message}</p>}
    </div>
    {offers.map((offer) => <div className="file-transfer__offer" role="dialog" aria-label={`Incoming file ${offer.name}`} key={offer.id}>
      <span className="eyebrow">INCOMING FILE</span><strong>{offer.name}</strong><span>{formatFileSize(offer.size)} · from {offer.from}</span>
      <div className="incoming-actions"><button className="ghost-button" type="button" disabled={offer.responding} onClick={() => void answer(offer, false)}>Reject</button><button className="primary-button" type="button" disabled={offer.responding} onClick={() => void answer(offer, true)}>Accept</button></div>
    </div>)}
    {transfers.length > 0 && <div className="file-transfer__list"><span className="eyebrow">TRANSFERS</span>{transfers.map((transfer) => <div className="file-transfer__row" key={transfer.id}>
      <div className="file-transfer__meta"><strong>{transfer.name}</strong><span>{transfer.direction} · {transfer.status === "error" ? transfer.error : transfer.status === "done" ? "Done" : `${formatFileSize(transfer.transferred)} / ${formatFileSize(transfer.total)}`}</span></div>
      <progress aria-label={`${transfer.name} progress`} value={transfer.total ? transfer.transferred : undefined} max={transfer.total || 1} />
    </div>)}</div>}
  </section>;
}
