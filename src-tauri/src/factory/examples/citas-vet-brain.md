---
proyecto: citas-vet
versión: 1
---

## Producto
- PRD-01 · DEBE · Una cita = cliente + mascota + veterinario + franja de 30 min.
- PRD-02 · DEBE · Fuera de alcance en v1: pagos y videollamada.

## Arquitectura
- ARQ-01 · DEBE · Capas: router → servicio → repositorio; nada se salta una capa.
  check: import-linter --config .importlinter
- ARQ-02 · DEBE · Los handlers no tocan la base de datos: pasan por un repositorio.
  check: ! grep -rn "session.query" app/routers
- ARQ-04 · DEBE · No se toca app/core/auth.py sin una persona.

## Tecnologías
- TEC-01 · DEBE · Python 3.12 · FastAPI · SQLAlchemy 2 · PostgreSQL 16.
- TEC-02 · DEBE · Librerías nuevas: solo de la lista permitida.
  check: python tools/check_deps.py

## Código
- COD-01 · DEBE · ruff y mypy sin errores.
  check: ruff check . && mypy app
- DEBERÍA · Funciones de menos de 40 líneas.

## Datos
- DAT-01 · DEBE · Toda tabla o columna nueva lleva migración Alembic.
  check: alembic check
- DAT-03 · DEBE · Nunca DELETE físico de citas: borrado lógico.

## Seguridad
- SEC-01 · DEBE · Toda entrada externa se valida; un cliente solo ve lo suyo.
- SEC-02 · DEBE · Ningún secreto en el repo.
  check: gitleaks detect

## Calidad
- CAL-01 · DEBE · «Hecho» = tests nuevos que cubren los criterios de aceptación.
- CAL-02 · DEBE · pytest en verde.
  check: pytest -q

## Git y entrega
- GIT-01 · DEBE · Rama feat/<clave>-<slug> desde dev.
- GIT-02 · DEBE · MR hacia dev con el enlace de ClickUp; nunca push a dev ni main.

## Despliegue
- DEP-01 · DEBE · CI de GitLab: lint, tests y migraciones en cada MR.
- DEBERÍA · Logs en JSON y health check en /health.

## Riesgo
- RSG-01 · DEBE · Parar y preguntar si cambia auth, pagos o el formato de datos.
- RSG-02 · DEBE · Una orden = una MR de menos de ~150 líneas.

## Decisiones
- DEC-01 · DEBERÍA · Borrado lógico de citas: la clínica necesita auditoría.
- DEC-02 · DEBERÍA · Franjas de 30 min fijas en v1.

## Pendiente
- ¿Zona horaria de las franjas: la de la clínica o UTC?
