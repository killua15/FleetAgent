import { useEffect, useState } from 'react';
import { homeDir } from '@tauri-apps/api/path';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { Spinner } from '../components/Spinner';
import { toastError, toastSuccess } from '../components/ErrorOverlay';
import type { FactoryProject } from '../types/factory';
import { ProjectSidebar } from './factory/ProjectSidebar';
import { ProjectHeader } from './factory/ProjectHeader';
import { Wizard } from './factory/Wizard';
import { StepProject } from './factory/StepProject';
import { StepBrain } from './factory/StepBrain';
import { StepTeam } from './factory/StepTeam';
import { StepTasks } from './factory/StepTasks';
import { StepPlay } from './factory/StepPlay';
import { Ops } from './factory/Ops';
import styles from './Factory.module.css';

/**
 * The Factory: projects whose tasks an automated line takes from order to
 * delivery. A project is set up once through the five-step wizard; after ▶
 * Play it shows the line at work.
 */
export function Factory() {
  const [projects, setProjects] = useState<FactoryProject[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [home, setHome] = useState<string | null>(null);
  const [step, setStep] = useState(1);
  /** Looking at the wizard of a project that's already running. */
  const [reconfiguring, setReconfiguring] = useState(false);

  useEffect(() => {
    homeDir()
      .then((h) => setHome(h.replace(/\/+$/, '')))
      .catch(() => setHome(null));
    invoke<FactoryProject[]>('factory_list_projects')
      .then((found) => {
        setProjects(found);
        setSelectedId(found[0]?.projectId ?? null);
        setStep(Math.min(5, found[0]?.setupStep ?? 1));
        setCreating(found.length === 0);
      })
      .catch((e) => toastError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  const selected = creating ? null : (projects.find((p) => p.projectId === selectedId) ?? null);

  const replace = (p: FactoryProject) => setProjects((prev) => (prev.some((x) => x.projectId === p.projectId) ? prev.map((x) => (x.projectId === p.projectId ? p : x)) : [...prev, p]));

  const select = (id: string) => {
    setCreating(false);
    setReconfiguring(false);
    setSelectedId(id);
    setStep(Math.min(5, projects.find((p) => p.projectId === id)?.setupStep ?? 1));
  };

  /** Moves the wizard, remembering how far the project got. */
  const go = async (n: number) => {
    setStep(n);
    if (!selected || n <= selected.setupStep) return;
    try {
      replace(await invoke<FactoryProject>('factory_update_project', { project: { ...selected, setupStep: n } }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const refresh = async (projectId: string) => {
    try {
      replace(await invoke<FactoryProject>('factory_get_project', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const stopAll = async () => {
    try {
      const n = await invoke<number>('factory_stop_all');
      toastSuccess(`⏹ Parado: ${n} proyecto${n === 1 ? '' : 's'}`);
      setProjects(await invoke<FactoryProject[]>('factory_list_projects'));
    } catch (e) {
      toastError(String(e));
    }
  };

  const renderStep = (p: FactoryProject | null) => {
    if (!p || step === 1) {
      return (
        <StepProject
          key={p?.projectId ?? 'new'}
          project={p}
          home={home}
          onCreated={(created) => {
            replace(created);
            setSelectedId(created.projectId);
            setCreating(false);
            setStep(1);
          }}
          onSaved={replace}
          onNext={() => go(2)}
        />
      );
    }
    if (step === 2) {
      return <StepTeam key={p.projectId} project={p} onBack={() => go(1)} onNext={() => go(3)} />;
    }
    if (step === 3) {
      return <StepBrain key={p.projectId} project={p} onSealed={() => refresh(p.projectId)} onBack={() => go(2)} onNext={() => go(4)} />;
    }
    if (step === 4) {
      return <StepTasks key={p.projectId} project={p} onBack={() => go(3)} onNext={() => go(5)} onToProvider={() => setStep(1)} />;
    }
    return (
      <StepPlay
        key={p.projectId}
        project={p}
        home={home}
        onBack={() => go(4)}
        onGoStep={(n) => setStep(n)}
        onStarted={() => {
          setReconfiguring(false);
          refresh(p.projectId);
        }}
      />
    );
  };

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.layout}>
        <ProjectSidebar projects={projects} selectedId={selectedId} creating={creating} onSelect={select} onNew={() => setCreating(true)} onStopAll={stopAll} />
        <main className={styles.main}>
          {loading ? (
            <div className={styles.empty}>
              <Spinner label="cargando…" />
            </div>
          ) : (
            <>
              <ProjectHeader project={selected} home={home} />
              {selected && selected.setupStep >= 6 && !reconfiguring ? (
                <Ops
                  key={selected.projectId}
                  project={selected}
                  onReconfigure={(n) => {
                    setReconfiguring(true);
                    setStep(n ?? 1);
                  }}
                  onChanged={() => refresh(selected.projectId)}
                />
              ) : (
                <Wizard step={selected ? step : 1} reached={selected ? Math.min(5, selected.setupStep) : 1} onGo={go}>
                  {renderStep(selected)}
                </Wizard>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
