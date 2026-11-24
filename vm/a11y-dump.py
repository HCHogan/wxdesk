"""Dump the AT-SPI tree of the running applications.

a11y-dump [--app NAME] [--depth N] [--max N] [--text N]
"""

import argparse
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402


def safe(f, default=None):
    try:
        return f()
    except Exception:
        return default


def states(acc):
    ss = safe(acc.get_state_set)
    if ss is None:
        return []
    keep = {
        "focused", "selected", "checked", "editable", "visible", "showing",
        "focusable", "selectable", "multi-line", "expanded", "collapsed",
        "defunct",
    }
    return [s.value_nick for s in ss.get_states() if s.value_nick in keep]


def text_of(acc, limit):
    t = safe(acc.get_text_iface)
    if t is None:
        return None
    n = safe(lambda: Atspi.Text.get_character_count(t), 0)
    if not n:
        return ""
    s = safe(lambda: Atspi.Text.get_text(t, 0, min(n, limit)), "")
    return s + ("…" if n > limit else "")


def actions_of(acc):
    a = safe(acc.get_action_iface)
    if a is None:
        return []
    n = safe(lambda: Atspi.Action.get_n_actions(a), 0)
    return [safe(lambda i=i: Atspi.Action.get_action_name(a, i), "?") for i in range(n)]


def line(acc, depth, args):
    role = safe(acc.get_role_name, "?")
    name = safe(acc.get_name, "") or ""
    desc = safe(acc.get_description, "") or ""
    r = safe(lambda: acc.get_extents(Atspi.CoordType.SCREEN))
    ext = f" @{r.x},{r.y} {r.width}x{r.height}" if r else ""
    parts = [f"{'  ' * depth}[{role}]"]
    if name:
        parts.append(repr(name))
    if desc:
        parts.append(f"desc={desc!r}")
    st = states(acc)
    if st:
        parts.append("{" + ",".join(st) + "}")
    parts.append(ext.strip())
    txt = text_of(acc, args.text)
    if txt and txt != name:
        parts.append(f"text={txt!r}")
    acts = actions_of(acc)
    if acts:
        parts.append(f"actions={acts}")
    attrs = safe(acc.get_attributes)
    if attrs:
        parts.append(f"attrs={dict(attrs)}")
    n = safe(acc.get_child_count, 0)
    if n:
        parts.append(f"children={n}")
    return " ".join(p for p in parts if p)


def walk(acc, depth, args, count):
    if count[0] >= args.max:
        return
    count[0] += 1
    print(line(acc, depth, args))
    if depth >= args.depth:
        return
    n = safe(acc.get_child_count, 0)
    for i in range(n):
        c = safe(lambda: acc.get_child_at_index(i))
        if c is not None:
            walk(c, depth + 1, args, count)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--app", help="only applications whose name contains this")
    p.add_argument("--depth", type=int, default=60)
    p.add_argument("--max", type=int, default=20000)
    p.add_argument("--text", type=int, default=200, help="characters of text shown")
    args = p.parse_args()

    desktop = Atspi.get_desktop(0)
    n = desktop.get_child_count()
    print(f"desktop: {n} applications", file=sys.stderr)
    count = [0]
    for i in range(n):
        app = safe(lambda: desktop.get_child_at_index(i))
        if app is None:
            continue
        name = safe(app.get_name, "") or ""
        print(f"app {i}: {name!r} children={safe(app.get_child_count, 0)}", file=sys.stderr)
        if args.app and args.app.lower() not in name.lower():
            continue
        walk(app, 0, args, count)
    print(f"{count[0]} nodes", file=sys.stderr)


if __name__ == "__main__":
    main()
