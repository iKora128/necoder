import net from 'node:net';
export function ipc(socketPath, method, params = {}, timeout = 12_000, auth) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let data = '', done = false;
    const finish = (error, result) => {
      if (done) return;
      done = true; socket.destroy();
      error ? reject(error) : resolve(result);
    };
    socket.setEncoding('utf8');
    socket.setTimeout(timeout, () => finish(new Error('ipc_timeout_outcome_unknown')));
    socket.on('error', error => finish(error));
    socket.on('end', () => finish(new Error('ipc_closed_outcome_unknown')));
    socket.on('connect', () => socket.write(JSON.stringify({ method, params, auth }) + '\n'));
    socket.on('data', chunk => {
      data += chunk;
      if (data.length > 2_000_000) return finish(new Error('ipc_response_too_large'));
      const newline = data.indexOf('\n');
      if (newline < 0) return;
      try {
        const response = JSON.parse(data.slice(0, newline));
        if (!response.ok) throw new Error(response.error || 'ipc_failed');
        finish(null, response.result);
      } catch (error) { finish(error); }
    });
  });
}
