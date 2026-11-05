#!/usr/bin/env node
//
// El cliente de prueba del puente: hace de teléfono desde la línea de comandos.
//
// Existe para que el escritorio se pueda terminar y verificar **antes** de que
// exista una app en Flutter. Habla el mismo protocolo (MOBILE_BRIDGE_TRD.md) con
// la misma criptografía, así que lo que funcione aquí funcionará allí — y lo que
// falle aquí falla en un sitio donde se puede depurar.
//
//   node contract/bridge-client.mjs login
//       Inicia sesión con Google. El canal es privado y solo deja entrar a la
//       cuenta dueña del Mac, así que esto hace falta una vez — igual que hará
//       la app en el móvil. La sesión queda en ~/.hangar-dev/bridge-session.json,
//       fuera del repositorio (ver SESSION_FILE); con HANGAR_DEV_SESSION se
//       puede poner en otro sitio.
//
//   node contract/bridge-client.mjs loopback
//       Dos conexiones al canal real, un sobre cifrado de una a otra. Prueba el
//       transporte sin necesitar Hangar abierto.
//
//   node contract/bridge-client.mjs pair '<json del QR>'
//       Se presenta a un Hangar abierto y espera la aprobación en el Mac.
//
//   node contract/bridge-client.mjs call '<json del QR>' list_projects
//       Ejecuta un comando y enseña la respuesta.

import { createClient } from '@supabase/supabase-js';
import sodium from 'libsodium-wrappers';
import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { homedir } from 'node:os';
// Con alias: este fichero ya tiene su propio `join`, el que se une al canal.
import { dirname, join as joinPath } from 'node:path';

// Los mismos que la app de escritorio: del entorno, o del `.env.local` del repo
// (ver `.env.example`). La clave publicable es pública por diseño; lo que
// protege el contenido es el cifrado, no ella.
function supabaseConfig() {
  const local = {};
  const file = joinPath(dirname(new URL(import.meta.url).pathname), '..', '.env.local');
  if (existsSync(file)) {
    for (const line of readFileSync(file, 'utf8').split('\n')) {
      const m = line.match(/^\s*([A-Z_]+)\s*=\s*(.*?)\s*$/);
      if (m) local[m[1]] = m[2];
    }
  }
  const url = process.env.VITE_SUPABASE_URL || local.VITE_SUPABASE_URL;
  const key = process.env.VITE_SUPABASE_PUBLISHABLE_KEY || local.VITE_SUPABASE_PUBLISHABLE_KEY;
  if (!url || !key) {
    console.error('Falta el proyecto de Supabase: pon VITE_SUPABASE_URL y VITE_SUPABASE_PUBLISHABLE_KEY en .env.local (ver .env.example).');
    process.exit(1);
  }
  return { url, key };
}
const { url: SUPABASE_URL, key: SUPABASE_KEY } = supabaseConfig();
const PROTOCOL_VERSION = 1;

// El mismo puerto que usa Hangar para su login, porque es el que está
// autorizado como redirect en Supabase. No chocan: el escritorio solo escucha
// durante los segundos de su propio intercambio.
const CALLBACK_PORT = 47823;

// Fuera del repositorio, a propósito. Estar en .gitignore evita que se suba,
// pero no evita las otras dos formas de filtrarlo: aparecer en pantalla cuando
// se enseña el árbol de ficheros — en un directo, en una captura, compartiendo
// pantalla — y colarse en un `zip` o un backup de la carpeta del proyecto. Un
// refresh token vale hasta que se usa o se revoca, así que basta con que se vea
// una vez.
const SESSION_FILE =
  process.env.HANGAR_DEV_SESSION ?? joinPath(homedir(), '.hangar-dev', 'bridge-session.json');

/** Guarda el token con el fichero cerrado a su dueño. */
function saveSession(refreshToken) {
  mkdirSync(dirname(SESSION_FILE), { recursive: true, mode: 0o700 });
  writeFileSync(SESSION_FILE, `${JSON.stringify({ refresh_token: refreshToken }, null, 2)}\n`, { mode: 0o600 });
  // `writeFileSync` solo aplica el modo al crear: sobre un fichero que ya
  // existía con permisos abiertos no haría nada.
  chmodSync(SESSION_FILE, 0o600);
}

await sodium.ready;

const b64 = (value) => sodium.from_base64(value, sodium.base64_variants.ORIGINAL);
const toB64 = (bytes) => sodium.to_base64(bytes, sodium.base64_variants.ORIGINAL);

function seal(key, body) {
  const nonce = sodium.randombytes_buf(sodium.crypto_secretbox_NONCEBYTES);
  const ciphertext = sodium.crypto_secretbox_easy(new TextEncoder().encode(JSON.stringify(body)), nonce, key);
  return { nonce: toB64(nonce), payload: toB64(ciphertext) };
}

function open(key, envelope) {
  const plaintext = sodium.crypto_secretbox_open_easy(b64(envelope.payload), b64(envelope.nonce), key);
  return JSON.parse(new TextDecoder().decode(plaintext));
}

function envelope(kind, key, body, id = randomUUID()) {
  return { v: PROTOCOL_VERSION, id, kind, ...seal(key, body) };
}

// ─────────────────────────────────────────────────────────── sesión

/**
 * Inicia sesión con Google, igual que hará la app en el móvil.
 *
 * El canal es privado: la política de `realtime.messages` solo deja entrar a la
 * cuenta que tiene ese Mac a su nombre. Así que este script necesita ser esa
 * cuenta — no hay atajo, y tenerlo es lo que hace que el script siga sirviendo
 * de suplente del teléfono.
 *
 * Guarda solo el refresh token. El de acceso dura una hora y se pide nuevo en
 * cada ejecución, así que no vale la pena tenerlo en un fichero.
 */
async function login() {
  const supabase = createClient(SUPABASE_URL, SUPABASE_KEY, { auth: { flowType: 'pkce', persistSession: false } });
  const redirectTo = `http://127.0.0.1:${CALLBACK_PORT}/callback`;

  const code = new Promise((resolve, reject) => {
    const server = createServer((req, res) => {
      const url = new URL(req.url, redirectTo);
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end('<p style="font:16px system-ui">Listo. Puedes cerrar esta pestaña.</p>');
      server.close();
      const value = url.searchParams.get('code');
      if (value) resolve(value);
      else reject(new Error(url.searchParams.get('error') ?? 'el navegador volvió sin código'));
    });
    server.on('error', reject);
    server.listen(CALLBACK_PORT, '127.0.0.1');
  });

  const { data, error } = await supabase.auth.signInWithOAuth({
    provider: 'google',
    options: { redirectTo, skipBrowserRedirect: true },
  });
  if (error) throw error;

  console.log('· abriendo el navegador para iniciar sesión con Google…');
  spawn('open', [data.url], { stdio: 'ignore', detached: true }).unref();

  const exchanged = await supabase.auth.exchangeCodeForSession(await code);
  if (exchanged.error) throw exchanged.error;

  saveSession(exchanged.data.session.refresh_token);
  console.log(`✓ sesión guardada para ${exchanged.data.session.user.email} en ${SESSION_FILE}`);
}

/** Un cliente con la sesión puesta, o un error que dice qué hacer. */
async function authenticated() {
  if (!existsSync(SESSION_FILE)) {
    throw new Error('sin sesión: corre `node contract/bridge-client.mjs login` primero — el canal es privado');
  }

  const saved = JSON.parse(readFileSync(SESSION_FILE, 'utf8'));
  const supabase = createClient(SUPABASE_URL, SUPABASE_KEY, { auth: { persistSession: false, autoRefreshToken: false } });

  const { data, error } = await supabase.auth.refreshSession({ refresh_token: saved.refresh_token });
  if (error || !data.session) throw new Error(`la sesión ya no vale, vuelve a hacer login: ${error?.message ?? 'sin sesión'}`);
  // El refresh token rota: guardar el nuevo o la próxima vez se reusa uno que
  // Supabase ya retiró.
  saveSession(data.session.refresh_token);

  // Realtime lleva su propio token. Sin esto la suscripción va como anónima y la
  // política la rechaza, con un error que no dice nada.
  supabase.realtime.setAuth(data.session.access_token);
  return { supabase, user: data.session.user, accessToken: data.session.access_token };
}

/**
 * Otro cliente con el mismo token.
 *
 * Con `broadcast: { self: false }` un cliente no recibe lo que él mismo manda,
 * así que el loopback necesita dos conexiones distintas. Comparten token en vez
 * de refrescar dos veces: cada refresco rota el refresh token y retira el
 * anterior, y hacerlo dos veces seguidas solo invita a dejar guardado uno muerto.
 */
function sibling(accessToken) {
  const supabase = createClient(SUPABASE_URL, SUPABASE_KEY, { auth: { persistSession: false, autoRefreshToken: false } });
  supabase.realtime.setAuth(accessToken);
  return supabase;
}

// ─────────────────────────────────────────────────────────── canal

/**
 * Se une al canal de una máquina y resuelve cuando está suscrito.
 *
 * `private: true` es lo que hace que Supabase consulte las políticas. Con el
 * canal público —como estaba— cualquiera que averiguase un `machine_id` podía
 * entrar: el cifrado protege el contenido, no el acceso.
 */
async function join(machineId, onMessage) {
  const { supabase } = await authenticated();
  return joinWith(supabase, machineId, onMessage);
}

async function joinWith(supabase, machineId, onMessage) {
  const channel = supabase.channel(`hangar:${machineId}`, { config: { private: true, broadcast: { self: false } } });

  if (onMessage) channel.on('broadcast', { event: 'msg' }, ({ payload }) => onMessage(payload));

  await new Promise((resolve, reject) => {
    channel.subscribe((status, err) => {
      if (status === 'SUBSCRIBED') resolve();
      if (status === 'CHANNEL_ERROR' || status === 'TIMED_OUT') {
        reject(new Error(`no se pudo unir al canal: ${status}${err ? ` — ${err.message}` : ''}`));
      }
    });
  });

  return { supabase, channel, send: (message) => channel.send({ type: 'broadcast', event: 'msg', payload: message }) };
}

/** Espera un sobre que cumpla una condición, o se rinde. */
function waitFor(predicate, seconds) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`nada llegó en ${seconds}s`)), seconds * 1000);
    predicate.resolve = (value) => {
      clearTimeout(timer);
      resolve(value);
    };
  });
}

// ─────────────────────────────────────────────────────────── loopback

/**
 * Un desconocido sin sesión no entra en ningún canal privado.
 *
 * Es la prueba de que el agujero está cerrado: hasta que el canal fue privado,
 * cualquiera que averiguase un `machine_id` se suscribía y veía pasar los sobres
 * —no podía abrirlos, pero sí contar cuántos, cuándo, y llenar el canal de
 * basura. Cifrar protege el contenido; esto protege el acceso.
 */
async function rechazaAnonimos() {
  const anon = createClient(SUPABASE_URL, SUPABASE_KEY, { auth: { persistSession: false } });
  const topic = `hangar:${randomUUID()}`;

  const status = await new Promise((resolve) => {
    const channel = anon.channel(topic, { config: { private: true } });
    channel.subscribe((s) => {
      if (['SUBSCRIBED', 'CHANNEL_ERROR', 'TIMED_OUT'].includes(s)) resolve(s);
    });
    setTimeout(() => resolve('SIN RESPUESTA'), 15000);
  });
  await anon.removeAllChannels();

  if (status === 'SUBSCRIBED') throw new Error(`un cliente sin sesión entró en ${topic}: el canal sigue abierto`);
  console.log(`✓ sin sesión no se entra en un canal privado (${status})`);
}

/**
 * Prueba el transporte y la autorización sin Hangar abierto.
 *
 * Lo que verifica y no se puede verificar de otra forma: que el proyecto de
 * Supabase acepta la conexión, que el broadcast llega al otro extremo, que el
 * sobre cifrado sobrevive el viaje intacto, y **que el canal solo deja entrar a
 * quien tiene esa máquina a su nombre**. Un test local no puede decir nada de
 * eso.
 *
 * Registra una máquina de usar y tirar y la borra al acabar: usar la de verdad
 * dejaría rastro en el canal que el escritorio está usando.
 */
async function loopback() {
  await rechazaAnonimos();

  const { supabase, user, accessToken } = await authenticated();
  console.log(`✓ sesión de ${user.email}`);

  const machineId = randomUUID();
  const key = sodium.randombytes_buf(sodium.crypto_secretbox_KEYBYTES);
  const arrival = {};

  const created = await supabase.from('machines').insert({ id: machineId, owner: user.id, name: 'loopback (prueba)' });
  if (created.error) throw new Error(`no se pudo registrar la máquina de prueba: ${created.error.message}`);

  try {
    const listener = await joinWith(supabase, machineId, (message) => arrival.resolve?.(message));
    const sender = await joinWith(sibling(accessToken), machineId);
    console.log(`✓ dos conexiones al canal privado hangar:${machineId}`);

    const sent = envelope('invoke', key, { name: 'list_projects', args: {} });
    await sender.send(sent);

    const received = await waitFor(arrival, 10);
    console.log('✓ el sobre llegó al otro extremo');

    const body = open(key, received);
    if (body.name !== 'list_projects') throw new Error(`el contenido no sobrevivió: ${JSON.stringify(body)}`);
    console.log(`✓ descifrado al otro lado: ${body.name}`);

    // Y lo que el relay ve: metadatos de enrutado y un blob que no puede abrir.
    console.log(`  Supabase transporta: ${JSON.stringify({ v: received.v, kind: received.kind, payload: `${received.payload.slice(0, 24)}…` })}`);

    // La otra mitad, y la que importa ahora: una máquina que no está a nombre de
    // nadie no tiene canal. Si esto **no** falla, el agujero sigue abierto.
    const ajeno = randomUUID();
    await joinWith(supabase, ajeno).then(
      () => {
        throw new Error(`el canal de una máquina sin registrar aceptó la suscripción: ${ajeno}`);
      },
      () => console.log('✓ un canal sin máquina registrada rechaza la suscripción'),
    );

    await listener.supabase.removeAllChannels();
    await sender.supabase.removeAllChannels();
  } finally {
    await supabase.from('machines').delete().eq('id', machineId);
  }
}

// ─────────────────────────────────────────────────────────── pair / call

async function talk(qrJson, command) {
  const qr = JSON.parse(qrJson);
  const key = b64(qr.k);
  const arrival = {};

  const link = await join(qr.m, (message) => arrival.resolve?.(message));
  console.log(`✓ unido al canal de ${qr.n ?? qr.m}`);

  await link.send(envelope('hello', key, { v: PROTOCOL_VERSION, device: 'script de prueba' }));
  const reply = open(key, await waitFor(arrival, 30));

  if (reply.code === 'E_PENDING_APPROVAL') {
    console.log('· Hangar está esperando que lo aceptes en el Mac. Escanear no es autorizar.');
    console.log('  (acepta el diálogo y vuelve a lanzar esto)');
    await link.supabase.removeAllChannels();
    return;
  }
  if (reply.code) throw new Error(`${reply.code}: ${reply.message}`);

  console.log(`✓ emparejado con «${reply.machineName}» · protocolo v${reply.v}`);
  console.log(`  puede pedir ${reply.commands.length} comandos`);

  if (!command) {
    await link.supabase.removeAllChannels();
    return;
  }

  const pending = {};
  arrival.resolve = (message) => pending.resolve?.(message);
  await link.send(envelope('invoke', key, { name: command, args: {} }));

  const result = open(key, await waitFor(pending, 30));
  console.log(`✓ ${command} →`, JSON.stringify(result).slice(0, 400));
  await link.supabase.removeAllChannels();
}

// ─────────────────────────────────────────────────────────── main

const [mode, ...rest] = process.argv.slice(2);
try {
  if (mode === 'login') await login();
  else if (mode === 'loopback') await loopback();
  else if (mode === 'pair') await talk(rest[0]);
  else if (mode === 'call') await talk(rest[0], rest[1]);
  else {
    console.log('uso: bridge-client.mjs login | loopback | pair <qr> | call <qr> <comando>');
    process.exit(2);
  }
  process.exit(0);
} catch (e) {
  console.error(`✗ ${e.message}`);
  process.exit(1);
}
