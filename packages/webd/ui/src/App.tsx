import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import {
  Activity, Radio as RadioIcon, TerminalSquare, BookOpen, Cpu, Wifi, Cable,
} from "lucide-react";
import {
  api, fmtUptime, serialWsUrl, type Board, type Radio, type SerialPort, type System,
} from "./api";

type View = "overview" | "radios" | "terminals";

export function App() {
  const [board, setBoard] = useState<Board | null>(null);
  const [view, setView] = useState<View>("overview");

  useEffect(() => { api.board().then(setBoard).catch(() => {}); }, []);

  return (
    <div style={{ display: "flex", height: "100%" }}>
      <Sidebar view={view} setView={setView} model={board?.model} />
      <main style={{ flex: 1, overflow: "auto", padding: "24px 28px" }}>
        {view === "overview" && <Overview board={board} />}
        {view === "radios" && <Radios radios={board?.radios ?? []} />}
        {view === "terminals" && <Terminals serials={board?.serials ?? []} />}
      </main>
    </div>
  );
}

function Sidebar({ view, setView, model }: { view: View; setView: (v: View) => void; model?: string }) {
  const items: [View, string, JSX.Element][] = [
    ["overview", "Overview", <Activity size={16} />],
    ["radios", "Radios", <RadioIcon size={16} />],
    ["terminals", "Terminals", <TerminalSquare size={16} />],
  ];
  return (
    <aside style={{
      width: "var(--sidebar-w)", borderRight: "1px solid var(--border)",
      background: "var(--bg-secondary)", display: "flex", flexDirection: "column", flexShrink: 0,
    }}>
      <div style={{ padding: "18px 18px 14px", borderBottom: "1px solid var(--border)" }}>
        <div style={{ fontFamily: "var(--font-display)", fontWeight: 800, fontSize: 18, letterSpacing: "-0.02em" }}>
          openHC
        </div>
        <div className="eyebrow" style={{ marginTop: 3 }}>{model ?? "controller"}</div>
      </div>
      <nav style={{ padding: 10, display: "flex", flexDirection: "column", gap: 2 }}>
        {items.map(([v, label, icon]) => (
          <button key={v} onClick={() => setView(v)}
            style={{
              display: "flex", alignItems: "center", gap: 10, textAlign: "left", border: "none",
              background: view === v ? "var(--accent-subtle)" : "transparent",
              color: view === v ? "var(--accent)" : "var(--text-secondary)",
              padding: "8px 10px", borderRadius: "var(--radius-sm)", fontWeight: 500,
            }}>
            {icon}{label}
          </button>
        ))}
        <a href="/api/openapi.json" target="_blank"
          style={{
            display: "flex", alignItems: "center", gap: 10, color: "var(--text-secondary)",
            padding: "8px 10px", fontWeight: 500,
          }}>
          <BookOpen size={16} />API
        </a>
      </nav>
    </aside>
  );
}

function Card({ title, children, right }: { title: string; children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <section style={{
      background: "var(--bg-secondary)", border: "1px solid var(--border)",
      borderRadius: "var(--radius-lg)", padding: 18, boxShadow: "var(--card-shadow)",
    }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 14 }}>
        <h2 className="eyebrow" style={{ margin: 0, fontSize: 10 }}>{title}</h2>
        {right}
      </div>
      {children}
    </section>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: "var(--radius-md)", padding: "12px 14px", flex: 1, minWidth: 120 }}>
      <div className="eyebrow">{label}</div>
      <div className="num" style={{ fontSize: 24, fontWeight: 300, marginTop: 4 }}>{value}</div>
    </div>
  );
}

function Overview({ board }: { board: Board | null }) {
  const [sys, setSys] = useState<System | null>(null);
  useEffect(() => {
    const t = setInterval(() => api.system().then(setSys).catch(() => {}), 4000);
    api.system().then(setSys).catch(() => {});
    return () => clearInterval(t);
  }, []);
  return (
    <div style={{ display: "grid", gap: 16, maxWidth: 860 }}>
      <Card title="System">
        <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
          <Stat label="uptime" value={sys ? fmtUptime(sys.uptime) : "—"} />
          <Stat label="load" value={sys ? sys.loadavg.split(" ").slice(0, 3).join(" ") : "—"} />
          <Stat label="kernel" value={sys?.kernel ?? "—"} />
        </div>
        <div style={{ marginTop: 14, display: "grid", gap: 6 }}>
          {sys?.interfaces.map((i) => (
            <div key={i.name} style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <span style={{ width: 8, height: 8, borderRadius: "50%", background: i.carrier ? "var(--ok)" : "var(--text-muted)" }} />
              <span className="mono" style={{ fontSize: 13 }}>{i.name}</span>
              <span className="mono num" style={{ fontSize: 13, color: "var(--text-secondary)" }}>{i.ip || "—"}</span>
            </div>
          ))}
        </div>
      </Card>
      <Card title="Hardware">
        <div style={{ display: "grid", gap: 10 }}>
          <Row icon={<Cpu size={15} />} label="model" value={board?.model ?? "—"} />
          <Row icon={<Wifi size={15} />} label="wifi" value={board?.wifi_iface || "none"} />
          <Row icon={<RadioIcon size={15} />} label="radios" value={board?.radios.map((r) => r.type).join(", ") || "none"} />
          <Row icon={<Cable size={15} />} label="serials" value={board?.serials.map((s) => s.label).join(", ") || "none"} />
        </div>
      </Card>
    </div>
  );
}

function Row({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
      <span style={{ color: "var(--text-muted)" }}>{icon}</span>
      <span className="eyebrow" style={{ width: 64 }}>{label}</span>
      <span className="mono" style={{ fontSize: 13 }}>{value}</span>
    </div>
  );
}

function Radios({ radios }: { radios: Radio[] }) {
  return (
    <div style={{ display: "grid", gap: 16, maxWidth: 700 }}>
      <p style={{ color: "var(--text-secondary)", fontSize: 13, margin: 0 }}>
        Raw UART transport + reset. Device pairing/control is a driver's job — this is what it builds on.
      </p>
      {radios.length === 0 && <Card title="Radios"><span style={{ color: "var(--text-muted)" }}>None on this board.</span></Card>}
      {radios.map((r) => <RadioPanel key={r.type} radio={r} />)}
    </div>
  );
}

function RadioPanel({ radio }: { radio: Radio }) {
  const [hex, setHex] = useState("");
  const [out, setOut] = useState<string | null>(null);
  return (
    <Card title={radio.type} right={<span className="mono" style={{ fontSize: 12, color: "var(--text-muted)" }}>{radio.dev}</span>}>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
        <input value={hex} onChange={(e) => setHex(e.target.value)} placeholder="hex to send, e.g. 1a c0 38 bc 7e" style={{ flex: 1, minWidth: 180 }} />
        <button className="accent" onClick={async () => {
          const r = await api.radioTx(radio.type, hex);
          setOut(r.error ? "error: " + r.error : `TX ${r.written} bytes`);
        }}>TX</button>
        <button onClick={async () => {
          setOut("listening 500ms…");
          const r = await api.radioRx(radio.type, 500);
          setOut(r.error ? "error: " + r.error : r.hex ? "RX " + r.hex : "RX (nothing)");
        }}>RX</button>
        <button onClick={async () => {
          const r = await api.radioReset(radio.type);
          setOut(r.reset ? "reset pulsed" : "reset failed");
        }}>Reset</button>
      </div>
      {out !== null && (
        <pre className="mono" style={{
          marginTop: 10, background: "var(--bg-tertiary)", border: "1px solid var(--border)",
          borderRadius: "var(--radius-sm)", padding: 10, fontSize: 12, whiteSpace: "pre-wrap", wordBreak: "break-all",
        }}>{out}</pre>
      )}
    </Card>
  );
}

function Terminals({ serials }: { serials: SerialPort[] }) {
  const [active, setActive] = useState<SerialPort | null>(null);
  useEffect(() => { if (!active && serials.length) setActive(serials[0]); }, [serials, active]);
  return (
    <div style={{ display: "grid", gap: 14, maxWidth: 900 }}>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
        {serials.map((s) => (
          <button key={s.dev} onClick={() => setActive(s)}
            className={active?.dev === s.dev ? "accent" : ""}>
            {s.label} · {s.dev.replace("/dev/", "")}
          </button>
        ))}
        {serials.length === 0 && <span style={{ color: "var(--text-muted)" }}>No serial ports declared.</span>}
      </div>
      {active && <SerialTerminal key={active.dev} port={active} />}
    </div>
  );
}

function SerialTerminal({ port }: { port: SerialPort }) {
  const ref = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState("connecting…");
  useEffect(() => {
    if (!ref.current) return;
    const term = new Terminal({
      fontFamily: "var(--font-mono)", fontSize: 13, cursorBlink: true,
      theme: { background: "#0b0d11", foreground: "#e6e9ef", cursor: "#34d399" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(ref.current);
    fit.fit();
    const onResize = () => fit.fit();
    window.addEventListener("resize", onResize);

    const ws = new WebSocket(serialWsUrl(port.dev));
    ws.binaryType = "arraybuffer";
    ws.onopen = () => setStatus(`${port.dev} @ ${port.baud}`);
    ws.onclose = () => setStatus("disconnected");
    ws.onerror = () => setStatus("error");
    ws.onmessage = (e) => term.write(new Uint8Array(e.data as ArrayBuffer));
    term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(new TextEncoder().encode(d));
    });
    return () => { window.removeEventListener("resize", onResize); ws.close(); term.dispose(); };
  }, [port.dev, port.baud]);
  return (
    <div>
      <div className="eyebrow" style={{ marginBottom: 6 }}>{status}</div>
      <div ref={ref} style={{ height: 380, background: "#0b0d11", border: "1px solid var(--border)", borderRadius: "var(--radius-md)", padding: 8 }} />
    </div>
  );
}
