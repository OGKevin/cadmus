# Vendored reviewdog formatters

Small JS formatters copied from upstream reviewdog actions so collect jobs can
emit `rdjson` without depending on those actions' post-to-PR behavior.

| Formatter | Upstream | Pinned commit |
| --------- | -------- | ------------- |
| `eslint-formatter-rdjson` | [reviewdog/action-eslint](https://github.com/reviewdog/action-eslint) | `556a3fdaf8b4201d4d74d406013386aa4f7dab96` (v1.34.0) |
| `stylelint-formatter-rdjson` | [reviewdog/action-stylelint](https://github.com/reviewdog/action-stylelint) | `086959bc5cd70db1b4954f45d8d396d9e3786bbb` (v1.31.0) |

Do not execute these from privileged `workflow_run` report jobs — they belong
in unprivileged collect workflows only.
