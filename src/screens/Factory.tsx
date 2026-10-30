import { useEffect, useState } from 'react';
import { homeDir } from '@tauri-apps/api/path';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import type { FactoryProject } from '../types/factory';
import { ProjectSidebar } from './factory/ProjectSidebar';
import { ProjectHeader } from './factory/ProjectHeader';
import { Wizard } from './factory/Wizard';
import { StepProject } from './factory/StepProject';
import styles from './Factory.module.css';

/**
 * The Factory: projects whose tasks an automated line takes from order to
 * delivery. F0 is its ground floor — the projects that joined and the first
 * setup step; the team, the brain, the tasks and the line arrive in F1–F5.
 */
export function Factory() {
  const [projects, setProjects] = useState<FactoryProject[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [home, setHome] = useState<string | null>(null);

  useEffect(() => {
    homeDir()
      .then((h) => setHome(h.replace(/\/+$/, '')))
      .catch(() => setHome(null));
    invoke<FactoryProject[]>('factory_list_projects')
      .then((found) => {
        setProjects(found);
        setSelectedId(found[0]?.projectId ?? null);
        setCreating(found.length === 0);
      })
      .catch((e) => toastError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  const selected = creating ? null : (projects.find((p) => p.projectId === selectedId) ?? null);

  const replace = (p: FactoryProject) => setProjects((prev) => (prev.some((x) => x.projectId === p.projectId) ? prev.map((x) => (x.projectId === p.projectId ? p : x)) : [...prev, p]));

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.layout}>
        <ProjectSidebar
          projects={projects}
          selectedId={selectedId}
          creating={creating}
          onSelect={(id) => {
            setCreating(false);
            setSelectedId(id);
          }}
          onNew={() => setCreating(true)}
        />
        <main className={styles.main}>
          {loading ? (
            <div className={styles.empty}>
              <Spinner label="cargando…" />
            </div>
          ) : (
            <>
              <ProjectHeader project={selected} home={home} />
              <Wizard>
                <StepProject
                  key={selected?.projectId ?? 'new'}
                  project={selected}
                  home={home}
                  onCreated={(p) => {
                    replace(p);
                    setSelectedId(p.projectId);
                    setCreating(false);
                  }}
                  onSaved={replace}
                />
              </Wizard>
            </>
          )}
        </main>
      </div>
    </div>
  );
}
