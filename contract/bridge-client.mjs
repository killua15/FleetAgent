#!/usr/bin/env node
//
// El cliente de prueba del puente: hace de teléfono desde la línea de comandos.
//
// Existe para que el escritorio se pueda terminar y verificar **antes** de que
// exista una app en Flutter. Habla el mismo protocolo (MOBILE_BRIDGE_TRD.md) con
// la misma criptografía, así que lo que funcione aquí funcionará allí — y lo que
// falle aquí falla en un sitio donde se puede depurar.
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

// Los mismos que la app de escritorio. La clave publicable es pública por
// diseño; lo que protege el contenido es el cifrado, no ella.
const SUPABASE_URL = 'https://yqrvofogmyqdqufyxlfk.supabase.co';
const SUPABASE_KEY = 'sb_publishable_CTwAWQ70NlE0dEzTpv2zhQ_M0k9SbiQ';
const PROTOCOL_VERSION = 1;

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

/** Se une al canal de una máquina y resuelve cuando está suscrito. */
async function join(machineId, onMessage) {
  const supabase = createClient(SUPABASE_URL, SUPABASE_KEY, { auth: { persistSession: false } });
  const channel = supabase.channel(`hangar:${machineId}`, { config: { broadcast: { self: false } } });

  if (onMessage) channel.on('broadcast', { event: 'msg' }, ({ payload }) => onMessage(payload));

  await new Promise((resolve, reject) => {
    channel.subscribe((status) => {
      if (status === 'SUBSCRIBED') resolve();
      if (status === 'CHANNEL_ERROR' || status === 'TIMED_OUT') reject(new Error(`no se pudo unir al canal: ${status}`));
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
 * Prueba el transporte sin Hangar: dos conexiones al canal de verdad, un sobre
 * cifrado de una a otra.
 *
 * Lo que verifica y no se puede verificar de otra forma: que el proyecto de
 * Supabase acepta la conexión, que el broadcast llega al otro extremo, y que el
 * sobre cifrado sobrevive el viaje intacto. Un test local no puede decir nada de
 * eso.
 */
async function loopback() {
  const machineId = `loopback-${randomUUID()}`;
  const key = sodium.randombytes_buf(sodium.crypto_secretbox_KEYBYTES);
  const arrival = {};

  const listener = await join(machineId, (message) => arrival.resolve?.(message));
  const sender = await join(machineId);
  console.log(`✓ dos conexiones al canal hangar:${machineId}`);

  const sent = envelope('invoke', key, { name: 'list_projects', args: {} });
  await sender.send(sent);

  const received = await waitFor(arrival, 10);
  console.log('✓ el sobre llegó al otro extremo');

  const body = open(key, received);
  if (body.name !== 'list_projects') throw new Error(`el contenido no sobrevivió: ${JSON.stringify(body)}`);
  console.log(`✓ descifrado al otro lado: ${body.name}`);

  // Y lo que el relay ve: metadatos de enrutado y un blob que no puede abrir.
  console.log(`  Supabase transporta: ${JSON.stringify({ v: received.v, kind: received.kind, payload: `${received.payload.slice(0, 24)}…` })}`);

  await listener.supabase.removeAllChannels();
  await sender.supabase.removeAllChannels();
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
  if (mode === 'loopback') await loopback();
  else if (mode === 'pair') await talk(rest[0]);
  else if (mode === 'call') await talk(rest[0], rest[1]);
  else {
    console.log('uso: bridge-client.mjs loopback | pair <qr> | call <qr> <comando>');
    process.exit(2);
  }
  process.exit(0);
} catch (e) {
  console.error(`✗ ${e.message}`);
  process.exit(1);
}
