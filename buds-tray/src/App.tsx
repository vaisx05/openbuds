import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type ConnectionState =
  | { state: "disconnected" }
  | { state: "lookingForConnectedDevice" }
  | { state: "scanning" }
  | { state: "connected"; deviceName: string }
  | { state: "failed"; reason: string };

type Battery = {
  left: number | null;
  right: number | null;
  case: number | null;
};

type AncMode = "off" | "on" | "trans";

const MODES: { key: AncMode; label: string; command: string }[] = [
  { key: "off", label: "Off", command: "anc_off" },
  { key: "on", label: "ANC", command: "anc_on" },
  { key: "trans", label: "Transparency", command: "anc_transparency" },
];

function statusMeta(cs: ConnectionState): {
  label: string;
  tone: "good" | "busy" | "bad" | "flat";
} {
  switch (cs.state) {
    case "disconnected":
      return { label: "Disconnected", tone: "flat" };
    case "lookingForConnectedDevice":
      return { label: "Looking for buds…", tone: "busy" };
    case "scanning":
      return { label: "Scanning…", tone: "busy" };
    case "connected":
      return { label: "Connected", tone: "good" };
    case "failed":
      return { label: "Connection failed", tone: "bad" };
  }
}

type Toast = { id: number; message: string; tone: "error" | "info" };
let nextToastId = 0;

function batteryTone(value: number | null): "good" | "mid" | "low" | "flat" {
  if (value === null) return "flat";
  if (value <= 20) return "low";
  if (value <= 50) return "mid";
  return "good";
}

function BatteryIcon({ value }: { value: number | null }) {
  const pct = value ?? 0;
  const tone = batteryTone(value);
  return (
    <svg
      className={`battery-icon tone-${tone}`}
      width="26"
      height="14"
      viewBox="0 0 26 14"
      fill="none"
    >
      <rect
        x="0.75"
        y="0.75"
        width="21.5"
        height="12.5"
        rx="3"
        stroke="currentColor"
        strokeOpacity="0.35"
        strokeWidth="1.5"
      />
      <rect x="23" y="4.5" width="2.2" height="5" rx="1" fill="currentColor" fillOpacity="0.35" />
      <rect
        x="2.5"
        y="2.5"
        width={Math.max(0, (17.5 * pct) / 100)}
        height="9"
        rx="1.8"
        fill="currentColor"
        className="battery-fill"
      />
    </svg>
  );
}

function ReconnectIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none">
      <path
        d="M4 4v6h6M20 20v-6h-6M4.5 15a8 8 0 0 0 14.5 3M19.5 9A8 8 0 0 0 5 6"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ToastStack({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="toast-stack">
      {toasts.map((t) => (
        <div key={t.id} className={`toast tone-${t.tone}`} onClick={() => onDismiss(t.id)}>
          {t.message}
        </div>
      ))}
    </div>
  );
}

function App() {
  const [connection, setConnection] = useState<ConnectionState>({
    state: "disconnected",
  });
  const [battery, setBattery] = useState<Battery>({
    left: null,
    right: null,
    case: null,
  });
  const [activeMode, setActiveMode] = useState<AncMode | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [showPhoneHint, setShowPhoneHint] = useState(false);

  function pushToast(message: string, tone: Toast["tone"] = "error") {
    const id = ++nextToastId;
    setToasts((t) => [...t, { id, message, tone }]);
    setTimeout(() => {
      setToasts((t) => t.filter((x) => x.id !== id));
    }, 5000);
  }

  function dismissToast(id: number) {
    setToasts((t) => t.filter((x) => x.id !== id));
  }

  useEffect(() => {
    const unlistenConnection = listen<ConnectionState>(
      "buds://connection-state",
      (event) => {
        setConnection(event.payload);
        if (event.payload.state === "failed") {
          pushToast(event.payload.reason);
        }
      },
    );
    const unlistenBattery = listen<Battery>("buds://battery", (event) =>
      setBattery(event.payload),
    );
    invoke<Battery | null>("get_battery").then((b) => {
      if (b) setBattery(b);
    });
    return () => {
      unlistenConnection.then((f) => f());
      unlistenBattery.then((f) => f());
    };
  }, []);

  const isConnected = connection.state === "connected";
  const hasBattery =
    battery.left !== null || battery.right !== null || battery.case !== null;
  const status = statusMeta(connection);

  // The buds don't respond to a battery *query* - they only push a status
  // update on their own, and that push has been confirmed to require a
  // phone (running the OnePlus app) also connected at the same time. So
  // "still —  after a while" almost always means "no phone connected",
  // not "broken" - surface that rather than leaving it silent.
  useEffect(() => {
    if (!isConnected || hasBattery) {
      setShowPhoneHint(false);
      return;
    }
    const timer = setTimeout(() => setShowPhoneHint(true), 12_000);
    return () => clearTimeout(timer);
  }, [isConnected, hasBattery]);

  async function setMode(mode: AncMode, command: string) {
    try {
      await invoke(command);
      setActiveMode(mode);
    } catch (e) {
      pushToast(String(e));
    }
  }

  async function reconnect() {
    setActiveMode(null);
    setBattery({ left: null, right: null, case: null });
    await invoke("reconnect");
  }

  const activeIndex = MODES.findIndex((m) => m.key === activeMode);

  return (
    <main className="container">
      <header className="header">
        <h1>OnePlus Buds 4</h1>
        <button className="icon-button" onClick={reconnect} title="Reconnect">
          <ReconnectIcon />
        </button>
      </header>

      <div className={`status-pill tone-${status.tone}`}>
        <span className="status-dot" />
        {status.label}
      </div>

      <section className="card battery-card">
        {(["left", "right", "case"] as const).map((key) => (
          <div className="battery-item" key={key}>
            <BatteryIcon value={battery[key]} />
            <span className="battery-value">
              {battery[key] === null ? "—" : `${battery[key]}%`}
            </span>
            <span className="battery-label">
              {key === "case" ? "Case" : key === "left" ? "Left" : "Right"}
            </span>
          </div>
        ))}
      </section>

      {showPhoneHint && (
        <p className="hint">
          Battery updates only arrive while your phone (with the OnePlus
          app) is also connected.
        </p>
      )}

      <section className="segmented" aria-disabled={!isConnected}>
        <div
          className="segmented-highlight"
          style={{
            transform: `translateX(${Math.max(activeIndex, 0) * 100}%)`,
            opacity: activeIndex >= 0 ? 1 : 0,
          }}
        />
        {MODES.map((mode) => (
          <button
            key={mode.key}
            className={activeMode === mode.key ? "active" : ""}
            disabled={!isConnected}
            onClick={() => setMode(mode.key, mode.command)}
          >
            {mode.label}
          </button>
        ))}
      </section>

      <ToastStack toasts={toasts} onDismiss={dismissToast} />
    </main>
  );
}

export default App;
