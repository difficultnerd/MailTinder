# Optional layer: OWASP ZAP (DAST)

Use for repos with a live web-facing login flow or API.

Install: `optional/install.sh zap` from the repo root.

1. Set repository variable `ZAP_TARGET_URL` (baseline, passive) and/or `ZAP_OPENAPI_URL` (API scan).
2. Point both at a staging deployment. The API scan sends attack traffic.
3. Tune `.zap/rules.tsv` as findings come in. Review reports in the workflow artifacts.
4. This workflow is not a required status check: it needs a deployed target and runs on a schedule.

Authenticated flows need a ZAP context and auth script; add them under `.zap/` and switch to
`zap-full-scan.py` or the automation framework once the login flow is stable.
