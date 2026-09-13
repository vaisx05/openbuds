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

function connectionLabel(cs: ConnectionState): string {
  switch (cs.state) {
    case "disconnected":
      return "Disconnected";
    case "lookingForConnectedDevice":
      return "Looking for connected buds…";
    case "scanning":
      return "Scanning for buds…";
    case "connected":
      return `Connected: ${cs.deviceName}`;
    case "failed":
      return `Failed: ${cs.reason}`;
  }
}

function batteryLabel(value: number | null): string {
  return value === null ? "—" : `${value}%`;
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
  const [error, setError] = useState<string | null>(null);
  const [showPhoneHint, setShowPhoneHint] = useState(false);

  useEffect(() => {
    const unlistenConnection = listen<ConnectionState>(
      "buds://connection-state",
      (event) => setConnection(event.payload),
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

  async function setMode(mode: AncMode) {
    setError(null);
    const command =
      mode === "on" ? "anc_on" : mode === "off" ? "anc_off" : "anc_transparency";
    try {
      await invoke(command);
      setActiveMode(mode);
    } catch (e) {
      setError(String(e));
    }
  }

  async function reconnect() {
    setError(null);
    setActiveMode(null);
    setBattery({ left: null, right: null, case: null });
    await invoke("reconnect");
  }

  return (
    <main className="container">
      <h1>OnePlus Buds 4</h1>

      <p className="status">{connectionLabel(connection)}</p>

      <div className="battery-row">
        <div className="battery-tile">
          <span className="battery-label">Left</span>
          <span className="battery-value">{batteryLabel(battery.left)}</span>
        </div>
        <div className="battery-tile">
          <span className="battery-label">Right</span>
          <span className="battery-value">{batteryLabel(battery.right)}</span>
        </div>
        <div className="battery-tile">
          <span className="battery-label">Case</span>
          <span className="battery-value">{batteryLabel(battery.case)}</span>
        </div>
      </div>

      {showPhoneHint && (
        <p className="hint">
          No battery data yet — the buds only push it while your phone
          (with the OnePlus app) is also connected.
        </p>
      )}

      <div className="anc-row">
        <button
          className={activeMode === "off" ? "active" : ""}
          disabled={!isConnected}
          onClick={() => setMode("off")}
        >
          Off
        </button>
        <button
          className={activeMode === "on" ? "active" : ""}
          disabled={!isConnected}
          onClick={() => setMode("on")}
        >
          ANC
        </button>
        <button
          className={activeMode === "trans" ? "active" : ""}
          disabled={!isConnected}
          onClick={() => setMode("trans")}
        >
          Transparency
        </button>
      </div>

      {error && <p className="error">{error}</p>}

      <button className="reconnect" onClick={reconnect}>
        Reconnect
      </button>
    </main>
  );
}

export default App;
