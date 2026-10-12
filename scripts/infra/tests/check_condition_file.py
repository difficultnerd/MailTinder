#!/usr/bin/env python3
"""Run a generated IAM condition through gcloud's own condition parser.

T-1116 (SEC-05). The bootstrap script writes each IAM condition to a JSON file
and hands it to gcloud with `--condition-from-file`. This helper feeds the same
content to the parser the installed Cloud SDK uses
(`ParseYamlOrJsonCondition` in `googlecloudsdk.command_lib.iam.iam_util`), so a
test proves the argument the script builds is one the real CLI accepts - without
a credential and without touching a cloud API.

With `--inline <string>` it instead feeds the string to gcloud's comma-delimited
`--condition=` parser (`_ConditionArgDict`), the form that rejected both
generated conditions because their expressions contain commas.

The SDK's own python modules have to be importable: run this with
PYTHONPATH=<sdk_root>/lib:<sdk_root>/lib/third_party.

Exit status is 0 when the parser accepts the input, non-zero otherwise.
"""
import sys


def _parse_condition_from_file(path):
    from googlecloudsdk.command_lib.iam import iam_util

    with open(path, encoding="utf-8") as handle:
        content = handle.read()
    condition = iam_util.ParseYamlOrJsonCondition(content)
    if not condition.get("expression") or not condition.get("title"):
        print("the parsed condition has no expression or title", file=sys.stderr)
        return 1
    if "modifiedGrantsByRole" not in condition["expression"]:
        print("the expression does not use modifiedGrantsByRole", file=sys.stderr)
        return 1
    return 0


def _parse_inline(value):
    from googlecloudsdk.command_lib.iam import iam_util

    # pylint: disable=protected-access
    iam_util._ConditionArgDict()(value)
    return 0


def main(argv):
    if len(argv) == 3 and argv[1] == "--inline":
        parser, value = _parse_inline, argv[2]
    elif len(argv) == 2:
        parser, value = _parse_condition_from_file, argv[1]
    else:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        return parser(value)
    except Exception as exc:  # any parser complaint is a rejection here
        print("%s: %s" % (type(exc).__name__, exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
