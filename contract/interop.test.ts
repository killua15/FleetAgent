import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { beforeAll, describe, expect, it } from 'vitest';
import sodium from 'libsodium-wrappers';

/**
 * La prueba que de verdad importa del cifrado del puente.
 *
 * Que Rust se entienda consigo mismo no dice nada sobre si el móvil podrá
 * hablar con el escritorio: `crypto_secretbox` se eligió **por interoperar**, y
 * eso solo se demuestra cruzando el lenguaje. Node hace aquí de suplente de
 * Dart — las dos usan libsodium de verdad, así que si Node abre lo de Rust,
 * `sodium_libs` también.
 *
 * Los ficheros se commitean, así que cada lado siempre tiene algo del otro
 * contra lo que verificarse, sin importar en qué orden se corran los tests.
 */
const FIXTURES = join(__dirname, 'fixtures');

const b64 = (value: string) => sodium.from_base64(value, sodium.base64_variants.ORIGINAL);

beforeAll(async () => {
  await sodium.ready;
});

describe('cifrado entre lenguajes', () => {
  it('Node abre lo que cifró Rust', () => {
    const fixture = JSON.parse(readFileSync(join(FIXTURES, 'crypto-rust.json'), 'utf8'));

    const plaintext = sodium.crypto_secretbox_open_easy(b64(fixture.ciphertext), b64(fixture.nonce), b64(fixture.key));

    expect(new TextDecoder().decode(plaintext)).toBe(fixture.plaintext);
  });

  it('y deja cifrado lo que Rust tiene que abrir', () => {
    const fixture = JSON.parse(readFileSync(join(FIXTURES, 'crypto-rust.json'), 'utf8'));
    const key = b64(fixture.key);
    const plaintext = '{"ok":true,"data":[]}';

    const nonce = sodium.randombytes_buf(sodium.crypto_secretbox_NONCEBYTES);
    const ciphertext = sodium.crypto_secretbox_easy(new TextEncoder().encode(plaintext), nonce, key);

    writeFileSync(
      join(FIXTURES, 'crypto-node.json'),
      `${JSON.stringify(
        {
          _comment: 'Producido por Node. Rust tiene que abrirlo — ver bridge::crypto::tests.',
          key: fixture.key,
          nonce: sodium.to_base64(nonce, sodium.base64_variants.ORIGINAL),
          ciphertext: sodium.to_base64(ciphertext, sodium.base64_variants.ORIGINAL),
          plaintext,
        },
        null,
        2,
      )}\n`,
    );

    // Comprobación local: si esto ya no cuadra, el fichero que acabamos de
    // escribir es basura y haría fallar a Rust por el motivo equivocado.
    expect(new TextDecoder().decode(sodium.crypto_secretbox_open_easy(ciphertext, nonce, key))).toBe(plaintext);
  });

  it('un mensaje con la clave equivocada no se abre', () => {
    const fixture = JSON.parse(readFileSync(join(FIXTURES, 'crypto-rust.json'), 'utf8'));
    const otra = sodium.randombytes_buf(sodium.crypto_secretbox_KEYBYTES);

    expect(() => sodium.crypto_secretbox_open_easy(b64(fixture.ciphertext), b64(fixture.nonce), otra)).toThrow();
  });
});
