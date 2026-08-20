/** The "Empieza aquí" checklist — see `src-tauri/src/commands/onboarding.rs`. */

export interface OnboardingStep {
  /** Matches a key in `STEP_COPY` below. Rust sends facts; the words live here. */
  id: string;
  done: boolean;
  /** The real number behind the tick. `null` on anything not done yet. */
  detail: string | null;
}

export interface OnboardingProgress {
  basics: OnboardingStep[];
  beyond: OnboardingStep[];
  basicsDone: number;
  basicsTotal: number;
}

/** What each step is, and — the part that matters — why anyone should care.
 *
 * `why` is not a description of the click. It is the reason the step exists at
 * all, which is what turns a checklist into an explanation of the product. A
 * step whose `why` could be deleted without loss should not be on the list.
 *
 * `where` names the screen to go and do it, since the whole point is that
 * nobody should have to hunt through 28 of them.
 */
export const STEP_COPY: Record<string, { title: string; why: string; where?: string }> = {
  requirements: {
    title: 'Las herramientas que hacen falta',
    why: 'Hangar no reimplementa a los agentes: los lanza. Sin su CLI en la máquina no hay nada que orquestar.',
    where: 'Ajustes → general',
  },
  first_account: {
    title: 'Tu primera cuenta',
    why: 'Cada cuenta vive en su propio directorio de configuración, así que dos cuentas nunca se pisan la sesión.',
    where: 'Cuentas',
  },
  first_project: {
    title: 'Un proyecto',
    why: 'Es el repositorio sobre el que trabajan los agentes, y de donde salen los worktrees.',
    where: 'Ajustes → proyectos',
  },
  first_worktree: {
    title: 'Una terminal en su propio worktree',
    why: 'Cada agente en una rama aparte: es lo que permite tener varios a la vez sin que se peleen por los mismos ficheros.',
    where: 'Terminales',
  },
  second_account: {
    title: 'Una segunda cuenta del mismo proveedor',
    why: 'Aquí empieza lo que solo hace Hangar. Con una sola cuenta no hay a dónde rotar cuando se agote.',
    where: 'Cuentas',
  },
  rotation_on: {
    title: 'Rotación activada en un proyecto',
    why: 'Le dices qué cuentas puede usar y en qué orden. La frontera trabajo/personal se aplica, no solo se avisa.',
    where: 'Ajustes → rotación',
  },
  has_rotated: {
    title: 'Ha rotado de verdad, y la conversación siguió',
    why: 'Este no se marca haciendo clic: ocurre solo cuando una cuenta se agota. Es la prueba del producto, no una tarea pendiente tuya.',
  },
  spend_visible: {
    title: 'El gasto, en dólares y por turno',
    why: 'No un porcentaje de cuota: lo que costó cada turno, en qué repo y con qué cuenta.',
    where: 'Uso',
  },
  budget_set: {
    title: 'Un límite de gasto',
    why: 'Un tope mensual por cuenta, o un reparto de la cuota semanal por proyecto. Frena el trabajo nuevo, nunca uno en marcha.',
    where: 'Ajustes → general',
  },
  queue_used: {
    title: 'Trabajo en cola esperando cuota',
    why: 'Dejas tareas encoladas y arrancan cuando vuelve la ventana, en vez de fallar contra ella.',
    where: 'Cola',
  },
  skills_on: {
    title: 'Skills activas en un proyecto',
    why: 'Llegan al agente como flags al arrancar. Nada se escribe dentro de tu repositorio.',
    where: 'Ajustes → skills',
  },
  pr_inbox: {
    title: 'El inbox de PRs',
    why: 'Los comentarios de revisión vuelven a la sesión que escribió el código, sin copiar y pegar.',
    where: 'PRs',
  },
  graph_built: {
    title: 'El grafo de código',
    why: 'Para que seis agentes no redescubran el repositorio uno por uno. Opcional: Hangar funciona sin él.',
    where: 'Ajustes → graphs',
  },
};
