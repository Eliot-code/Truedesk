import { useEffect, useState } from "react";
import * as api from "../lib/api";
import { friendlyError, useNotify } from "../lib/notify";

export interface WakePanelProps {
  open: boolean;
  /** The computer this session views; it sends the wake-up signal. */
  currentId: string;
  onClose: () => void;
}

interface Device { id: string; name: string }

/** Wake computers that sleep on the viewed computer's network, through it. */
export default function WakePanel({ open, currentId, onClose }: WakePanelProps) {
  const notify = useNotify();
  const [devices, setDevices] = useState<Device[]>([]);

  useEffect(() => {
    if (!open) return;
    let active = true;
    void Promise.all([api.listWakeable(), api.listContacts()]).then(([ids, contacts]) => {
      if (!active) return;
      const names = new Map(contacts.map((contact) => [contact.id, contact.alias]));
      setDevices(ids.filter((id) => id !== currentId).map((id) => ({ id, name: names.get(id) ?? id })).sort((a, b) => a.name.localeCompare(b.name)));
    }).catch(() => {});
    return () => { active = false; };
  }, [open, currentId]);

  const wake = (device: Device) => {
    void api.wakeDevice(device.id)
      .then(() => notify(`Wake-up signal sent to ${device.name}. It can take a minute to come online.`, "info"))
      .catch((error: unknown) => notify(friendlyError(error)));
  };

  return (
    <section className="wake-panel" aria-label="Wake devices" hidden={!open}>
      <div className="wake-panel__header"><span className="eyebrow">WAKE-ON-LAN</span><button className="icon-button wake-panel__close" type="button" onClick={onClose} aria-label="Close wake devices">×</button></div>
      <p className="wake-panel__hint">Wake a computer that sleeps on the same network as {currentId}. Both this computer and that one send the wake-up signal. The sleeping computer needs Wake-on-LAN turned on.</p>
      {devices.length > 0
        ? <ul className="wake-panel__list">
            {devices.map((device) => (
              <li className="wake-panel__item" key={device.id}>
                <span>{device.name}{device.name !== device.id && <small className="mono">{device.id}</small>}</span>
                <button className="ghost-button" type="button" onClick={() => wake(device)} aria-label={`Wake ${device.name}`}>Wake</button>
              </li>
            ))}
          </ul>
        : <p className="wake-panel__empty">No devices to wake yet. Truedesk learns how to wake a computer while you're connected to it.</p>}
    </section>
  );
}
