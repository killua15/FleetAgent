import { describe, expect, it } from 'vitest';
import { lastSegment, providerShort, tildePath, type FactoryProject } from './factory';

const project = (over: Partial<FactoryProject>): FactoryProject => ({
  projectId: 'p1',
  projectName: 'Citas Vet',
  projectPath: '/Users/me/Proyects/citas-vet',
  providerKind: 'none',
  providerSourceId: null,
  providerScopeId: null,
  providerScopeLabel: null,
  delivery: 'local',
  mode: 'supervised',
  appStartCmd: null,
  appUrl: null,
  setupStep: 1,
  loopState: 'stopped',
  createdAt: '',
  updatedAt: '',
  ...over,
});

describe('factory labels', () => {
  it('names the provider by its list, not its whole path', () => {
    expect(providerShort(project({ providerKind: 'clickup', providerScopeLabel: 'Clínica / Proyectos / Citas Vet' }))).toBe('ClickUp «Citas Vet»');
    expect(providerShort(project({ providerKind: 'github', providerScopeLabel: 'clinica/citas-vet' }))).toBe('GitHub «clinica/citas-vet»');
    expect(providerShort(project({}))).toBe('local (Hangar)');
    expect(lastSegment('Tasks')).toBe('Tasks');
  });

  it('shortens paths under home and leaves the rest alone', () => {
    expect(tildePath('/Users/me/Proyects/citas-vet', '/Users/me')).toBe('~/Proyects/citas-vet');
    expect(tildePath('/Users/meow/x', '/Users/me')).toBe('/Users/meow/x');
    expect(tildePath('/opt/x', null)).toBe('/opt/x');
  });
});
