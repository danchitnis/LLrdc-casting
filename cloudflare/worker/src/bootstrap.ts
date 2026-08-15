import { BOOTSTRAP_LIMITS, CRYPTO_CONFIG, PAIRING_CODE_LENGTH, REQUEST_LIMITS } from "./config";

const BOOTSTRAP_HTML = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <link rel="icon" href="data:,">
  <title>LLrdc Pairing</title>
  <style>
    :root { color-scheme: dark; font-family: system-ui, sans-serif; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #0b0f17; color: #f1f5f9; }
    main { width: min(22rem, calc(100% - 2rem)); padding: 1.5rem; border: 1px solid #23304a; border-radius: 1rem; background: #131b2e; }
    h1 { margin: 0 0 .5rem; font-size: 1.25rem; }
    p { color: #94a3b8; line-height: 1.45; }
    form { display: flex; gap: .5rem; }
    input, button { min-height: 2.75rem; border-radius: .5rem; border: 1px solid #334155; font: inherit; }
    input { min-width: 0; flex: 1; padding: 0 .75rem; color: #fff; background: #0b1220; text-align: center; text-transform: uppercase; letter-spacing: .3em; font-weight: 700; }
    button { padding: 0 1rem; color: #fff; background: #0284c7; font-weight: 700; }
    button:disabled { opacity: .6; }
    #status { min-height: 1.4em; margin-bottom: 0; font-size: .9rem; }
  </style>
</head>
<body>
  <main>
    <h1>Connect to receiver</h1>
    <p>Enter the ${PAIRING_CODE_LENGTH}-character code shown on the receiver HDMI screen.</p>
    <form id="pair-form">
      <input id="pair-code" inputmode="text" pattern="[A-Za-z0-9]{${PAIRING_CODE_LENGTH}}" maxlength="${PAIRING_CODE_LENGTH}" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" placeholder="A78Q" required>
      <button id="pair-button" type="submit">Connect</button>
    </form>
    <p id="status" role="status">Waiting for code</p>
  </main>
  <script>
    (() => {
      const form = document.getElementById('pair-form');
      const codeInput = document.getElementById('pair-code');
      const button = document.getElementById('pair-button');
      const status = document.getElementById('status');

      codeInput.addEventListener('input', () => {
        codeInput.value = codeInput.value.toUpperCase();
      });

      function decodeHex(value) {
        if (!new RegExp('^[0-9a-f]{${REQUEST_LIMITS.CERTIFICATE_HASH_HEX_LENGTH}}$', 'i').test(value)) return null;
        const bytes = new Uint8Array(${BOOTSTRAP_LIMITS.CERTIFICATE_HASH_BYTES});
        for (let i = 0; i < bytes.length; i += 1) {
          const start = i * ${CRYPTO_CONFIG.HEX_CHARS_PER_BYTE};
          bytes[i] = parseInt(value.slice(start, start + ${CRYPTO_CONFIG.HEX_CHARS_PER_BYTE}), 16);
        }
        return bytes;
      }

      function frame(payload) {
        const bytes = new TextEncoder().encode(JSON.stringify(payload));
        const result = new Uint8Array(bytes.length + ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES});
        new DataView(result.buffer).setUint32(0, bytes.length, false);
        result.set(bytes, ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES});
        return result;
      }

      async function readUi(stream) {
        const reader = stream.readable.getReader();
        let pending = new Uint8Array(0);
        while (true) {
          const part = await reader.read();
          if (part.done) throw new Error('UI transfer ended early');
          const merged = new Uint8Array(pending.length + part.value.length);
          merged.set(pending);
          merged.set(part.value, pending.length);
          pending = merged;
          if (pending.length < ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES}) continue;
          const length = new DataView(pending.buffer, pending.byteOffset, ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES}).getUint32(0, false);
          if (length === 0 || length > ${BOOTSTRAP_LIMITS.MAX_UI_BYTES}) throw new Error('Invalid UI response');
          if (pending.length < length + ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES}) continue;
          return new TextDecoder().decode(pending.slice(${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES}, length + ${BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES}));
        }
      }

      async function loadLocalApp(result, code) {
        const certHash = decodeHex(result.cert_hash_hex);
        if (!certHash || !result.ip_address || !result.webtransport_port || !result.connection_token) {
          throw new Error('Invalid pairing response');
        }
        const query = new URLSearchParams({ code, token: result.connection_token });
        const transport = new WebTransport(
          'https://' + result.ip_address + ':' + result.webtransport_port + '/?' + query,
          { serverCertificateHashes: [{ algorithm: 'sha-256', value: certHash.buffer }] },
        );
        await transport.ready;
        const uiStream = await transport.createBidirectionalStream();
        const writer = uiStream.writable.getWriter();
        await writer.write(frame({ type: 'get_ui' }));
        await writer.close();
        const html = await readUi(uiStream);
        window.__LLRDC_BOOTSTRAP_CONNECTION__ = {
          ip: result.ip_address, port: result.webtransport_port,
          certHash: result.cert_hash_hex, code, token: result.connection_token,
        };
        window.__LLRDC_BOOTSTRAP_TRANSPORT__ = transport;
        document.open();
        document.write(html);
        document.close();
      }

      form.addEventListener('submit', async (event) => {
        event.preventDefault();
        const code = codeInput.value.trim().toUpperCase();
        if (!/^[A-Z0-9]{${PAIRING_CODE_LENGTH}}$/.test(code)) { status.textContent = 'Enter ${PAIRING_CODE_LENGTH} letters or numbers'; return; }
        button.disabled = true;
        status.textContent = 'Pairing';
        try {
          const response = await fetch('/api/pair', {
            method: 'POST', headers: { 'content-type': 'application/json' },
            cache: 'no-store', body: JSON.stringify({ code }),
          });
          if (!response.ok) throw new Error('Code invalid or receiver unavailable');
          await loadLocalApp(await response.json(), code);
        } catch (error) {
          status.textContent = error instanceof Error ? error.message : 'Pairing failed';
          button.disabled = false;
        }
      });
    })();
  </script>
</body>
</html>`;

export function bootstrapResponse(): Response {
  return new Response(BOOTSTRAP_HTML, {
    headers: { "cache-control": "no-store", "content-type": "text/html; charset=utf-8" },
  });
}
