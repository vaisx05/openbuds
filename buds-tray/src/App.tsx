import { useEffect, useState, type ReactElement } from "react";
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

function OffIcon() {
  return (
    <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="2" />
      <path d="M5.6 5.6l12.8 12.8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

function AncIcon() {
  return (
    <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
      <path
        d="M3 12h2M8 7v10M12 4v16M16 8v8M20 11v2"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function TransparencyIcon() {
  return (
    <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="2.2" stroke="currentColor" strokeWidth="2" />
      <path
        d="M8.2 8.2a5.4 5.4 0 0 0 0 7.6M15.8 8.2a5.4 5.4 0 0 1 0 7.6M5.4 5.4a9.4 9.4 0 0 0 0 13.2M18.6 5.4a9.4 9.4 0 0 1 0 13.2"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

const MODES: {
  key: AncMode;
  label: string;
  command: string;
  Icon: () => ReactElement;
}[] = [
  { key: "off", label: "Off", command: "anc_off", Icon: OffIcon },
  { key: "on", label: "ANC", command: "anc_on", Icon: AncIcon },
  {
    key: "trans",
    label: "Transparency",
    command: "anc_transparency",
    Icon: TransparencyIcon,
  },
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

const RING_SIZE = 86;
const RING_STROKE = 8;
const RING_R = (RING_SIZE - RING_STROKE) / 2;
const RING_C = 2 * Math.PI * RING_R;

function BatteryRing({ value }: { value: number | null }) {
  const pct = Math.min(100, Math.max(0, value ?? 0));
  const tone = batteryTone(value);
  return (
    <div className={`battery-ring tone-${tone}`}>
      <svg width={RING_SIZE} height={RING_SIZE} viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`}>
        <circle
          className="ring-track"
          cx={RING_SIZE / 2}
          cy={RING_SIZE / 2}
          r={RING_R}
          fill="none"
          strokeWidth={RING_STROKE}
        />
        <circle
          className="ring-fill"
          cx={RING_SIZE / 2}
          cy={RING_SIZE / 2}
          r={RING_R}
          fill="none"
          strokeWidth={RING_STROKE}
          strokeLinecap="round"
          strokeDasharray={`${(RING_C * pct) / 100} ${RING_C}`}
          transform={`rotate(-90 ${RING_SIZE / 2} ${RING_SIZE / 2})`}
        />
      </svg>
      <span className="battery-value">{value === null ? "—" : `${value}%`}</span>
    </div>
  );
}

function BudsIcon() {
  return (
    <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
      <path
        d="M8 3.5c-2 0-3.4 1.7-3.4 4 0 2 .9 3.4 2 4.2v6.3a1.6 1.6 0 0 0 3.2 0V7.5C9.8 5.2 9 3.5 8 3.5zM16 3.5c2 0 3.4 1.7 3.4 4 0 2-.9 3.4-2 4.2v6.3a1.6 1.6 0 0 1-3.2 0V7.5c0-2.3.8-4 1.8-4z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ReconnectIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none">
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
    // The buds echo their active mode on every change (ours, the phone's, or
    // a touch gesture), so this keeps the highlight in sync with reality.
    const unlistenAnc = listen<AncMode | null>("buds://anc-mode", (event) =>
      setActiveMode(event.payload),
    );
    invoke<AncMode | null>("get_anc_mode").then((m) => {
      if (m) setActiveMode(m);
    });
    invoke<Battery | null>("get_battery").then((b) => {
      if (b) setBattery(b);
    });
    return () => {
      unlistenConnection.then((f) => f());
      unlistenBattery.then((f) => f());
      unlistenAnc.then((f) => f());
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
    // Optimistic: highlight immediately, roll back only on a hard failure.
    // The buds' mode echo (listener above) corrects it if it differs.
    const previous = activeMode;
    setActiveMode(mode);
    try {
      await invoke(command);
    } catch (e) {
      setActiveMode(previous);
      pushToast(String(e));
    }
  }

  async function reconnect() {
    setActiveMode(null);
    setBattery({ left: null, right: null, case: null });
    await invoke("reconnect");
  }

  return (
    <main className="container">
      <div className="panel">
        <header className="header">
          <div className="device-icon">
            <BudsIcon />
          </div>
          <div className="device-info">
            <h1>OnePlus Buds 4</h1>
            <div className={`status tone-${status.tone}`}>
              <span className="status-dot" />
              {status.label}
            </div>
          </div>
          <button className="icon-button" onClick={reconnect} title="Reconnect">
            <ReconnectIcon />
          </button>
        </header>

        <div className={`hero ${isConnected ? "is-open" : "is-closed"}`}>
          <img className="hero-img hero-closed" src="/buds4_closed.png" alt="" draggable={false} />
          <img
            className="hero-img hero-opened"
            src="/buds4_opened.png"
            alt="OnePlus Buds 4 case"
            draggable={false}
          />
        </div>

        <section className="card battery-card">
          <h2>Battery</h2>
          <div className="battery-row">
            {(["left", "right", "case"] as const).map((key) => (
              <div className="battery-item" key={key}>
                <BatteryRing value={battery[key]} />
                <span className="battery-label">{key}</span>
              </div>
            ))}
          </div>
        </section>

        {showPhoneHint && (
          <p className="hint">
            Battery updates only arrive while your phone (with the OnePlus
            app) is also connected.
          </p>
        )}

        <h2 className="section-title">Noise control</h2>
        <section className="modes" aria-disabled={!isConnected}>
          {MODES.map(({ key, label, command, Icon }) => (
            <button
              key={key}
              className={`mode ${activeMode === key ? "active" : ""}`}
              disabled={!isConnected}
              onClick={() => setMode(key, command)}
            >
              <Icon />
              <span>{label}</span>
            </button>
          ))}
        </section>
      </div>

      <ToastStack toasts={toasts} onDismiss={dismissToast} />
    </main>
  );
}

export default App;
