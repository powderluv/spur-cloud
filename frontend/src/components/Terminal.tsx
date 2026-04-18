import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';

interface Props {
  wsUrl: string;
}

export default function Terminal({ wsUrl }: Props) {
  const termRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!termRef.current) return;

    const term = new XTerm({
      theme: {
        background: '#0d1117',
        foreground: '#c9d1d9',
        cursor: '#58a6ff',
        selectionBackground: '#264f78',
      },
      fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      fontSize: 14,
      cursorBlink: true,
      // Issue #10: explicit buffer/scroll config for cross-platform compatibility
      scrollback: 5000,
      convertEol: true,
    });
    xtermRef.current = term;

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());

    term.open(termRef.current);
    fitAddon.fit();

    // Connect WebSocket
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer'; // Handle binary data properly
    wsRef.current = ws;

    ws.onopen = () => {
      term.writeln('\x1b[32mConnected to session.\x1b[0m\r\n');
    };

    ws.onmessage = (event) => {
      if (typeof event.data === 'string') {
        term.write(event.data);
      } else {
        // Handle binary data (ArrayBuffer)
        term.write(new Uint8Array(event.data));
      }
    };

    ws.onclose = (event) => {
      if (event.wasClean) {
        term.writeln('\r\n\x1b[33mSession ended.\x1b[0m');
      } else {
        term.writeln('\r\n\x1b[31mConnection lost.\x1b[0m');
      }
    };

    ws.onerror = () => {
      term.writeln('\r\n\x1b[31mConnection error.\x1b[0m');
    };

    // Send keystrokes to WebSocket
    term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(data);
      }
    });

    // Issue #10: WebSocket keepalive ping every 30s to prevent idle timeout
    const pingInterval = setInterval(() => {
      if (ws.readyState === WebSocket.OPEN) {
        // Send empty ping to keep connection alive
        // Some proxies (nginx) close idle WebSocket connections
        ws.send('');
      }
    }, 30000);

    // Handle resize
    const resizeObserver = new ResizeObserver(() => {
      fitAddon.fit();
    });
    resizeObserver.observe(termRef.current);

    return () => {
      clearInterval(pingInterval);
      resizeObserver.disconnect();
      ws.close();
      term.dispose();
    };
  }, [wsUrl]);

  return (
    <div
      ref={termRef}
      className="w-full h-[600px] bg-[#0d1117] rounded-lg overflow-hidden border border-gray-800"
    />
  );
}
