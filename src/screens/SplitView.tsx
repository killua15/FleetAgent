import { useState } from 'react';
import { useNavigation } from '../nav/NavigationProvider';
import { Button } from '../components/ui/button';
import { AgentPanelBody } from './AgentPanel';
import styles from './SplitView.module.css';

const MAX_PANES = 4;

export function SplitView() {
  const { current, back } = useNavigation();
  const requestedIds = (current.params?.sessionIds as string[] | undefined) ?? [];
  const [paneIds, setPaneIds] = useState<string[]>(requestedIds.slice(0, MAX_PANES));

  const removePane = (id: string) => setPaneIds((prev) => prev.filter((p) => p !== id));

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        <Button variant="outline" size="icon" onClick={back} title="Back to workspace grid">
          ←
        </Button>
        <span className={styles.title}>Split view · {paneIds.length} session{paneIds.length === 1 ? '' : 's'}</span>
        {requestedIds.length > MAX_PANES && (
          <span className={styles.note}>showing the first {MAX_PANES} of {requestedIds.length} selected</span>
        )}
      </div>

      {paneIds.length === 0 ? (
        <div className={styles.empty}>
          <span>no sessions left in this split view</span>
          <Button variant="outline" onClick={back}>
            ← back to grid
          </Button>
        </div>
      ) : (
        <div
          className={styles.grid}
          style={{ gridTemplateColumns: paneIds.length === 1 ? 'minmax(0, 1fr)' : 'minmax(0, 1fr) minmax(0, 1fr)' }}
        >
          {paneIds.map((id) => (
            <div className={styles.paneWrap} key={id}>
              <AgentPanelBody
                sessionId={id}
                embedded
                onClose={() => removePane(id)}
                closeIcon="×"
                closeTitle="Remove from split view"
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
