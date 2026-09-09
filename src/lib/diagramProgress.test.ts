import { describe, expect, it } from 'vitest';
import { advance, finish, IDLE, type DiagramProgress } from './diagramProgress';

/** Replays a stream of blocks, which is how this is actually used. */
const run = (blocks: Array<[string, string]>): DiagramProgress =>
  blocks.reduce((p, [kind, text]) => advance(p, { kind, text }), IDLE);

describe('diagramProgress', () => {
  it('claims a stage only once its evidence has gone past', () => {
    // Reading and thinking are not milestones, however much of them there is.
    const early = run([
      ['think', 'voy a mirar el esquema'],
      ['out', 'architecture.schema.json'],
    ]);
    expect(early.stage).toBe('starting');
  });

  it('follows the real shape of an archify run', () => {
    const p = run([
      ['edit', 'Write  /tmp/diagrams/mapa.architecture.json'],
      ['cmd', '$ node bin/archify.mjs validate architecture mapa.architecture.json --quality showcase'],
      ['cmd', '$ node bin/archify.mjs deliver architecture mapa.architecture.json mapa.html'],
    ]);
    expect(p.stage).toBe('delivering');
    expect(p.validationRounds).toBe(1);
    expect(finish(p).stage).toBe('done');
  });

  it('counts validation rounds, which is the number that tells you it is working', () => {
    // The diagram in this repo took four rounds. Without the count, "comprobando"
    // for six minutes is indistinguishable from stuck.
    const p = run([
      ['edit', 'Write  spec.architecture.json'],
      ['cmd', '$ node archify.mjs validate architecture spec.json'],
      ['cmd', '$ node archify.mjs validate architecture spec.json'],
      ['cmd', '$ node archify.mjs validate architecture spec.json'],
    ]);
    expect(p.validationRounds).toBe(3);
    expect(p.stage).toBe('validating');
  });

  it('goes back to comprobando when a delivery fails, because that is what happens', () => {
    const p = run([
      ['cmd', '$ node archify.mjs validate architecture spec.json'],
      ['cmd', '$ node archify.mjs deliver architecture spec.json out.html'],
      ['cmd', '$ node archify.mjs validate architecture spec.json'],
    ]);
    expect(p.stage).toBe('validating');
    expect(p.validationRounds).toBe(2);
  });

  it('does not call a run finished just because it stopped', () => {
    // A turn that dies halfway also stops. Only reaching `deliver` counts.
    expect(finish(run([['edit', 'Write  spec.architecture.json']])).stage).toBe('failed');
    expect(finish(IDLE).stage).toBe('failed');
  });

  it('never mistakes another tool for archify', () => {
    const p = run([
      ['cmd', '$ git status'],
      ['cmd', '$ npm run validate'],
      ['cmd', '$ ./deliver.sh'],
    ]);
    expect(p.stage).toBe('starting');
    expect(p.validationRounds).toBe(0);
    // But the line still moves, so the panel is not frozen.
    expect(p.lastAction).toBe('./deliver.sh');
  });

  it('keeps the action line to one short line', () => {
    const p = advance(IDLE, { kind: 'agent', text: `una respuesta\ncon saltos\ny ${'muy '.repeat(40)}larga` });
    expect(p.lastAction).not.toContain('\n');
    expect(p.lastAction!.length).toBeLessThanOrEqual(60);
  });
});
