#!/usr/bin/env python3
"""Run the mod in a real game and check the files it writes.

    python3 tools/proof.py --bin target/horizon-cargo/fast --resources /path/to/prepared/resources

Needs `horizon-ctl` beside the built `horizon-client`. Everything happens in temporary folders:
never in a player's real saves. Add --open to also press "Open profile location", which opens a
file manager window. Exit status 0 when every check passed.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

KINDS = {
    "session": "events", "dimension_change": "events", "player_state": "events",
    "block_broken": "events", "block_placed": "events", "player_deaths": "events",
    "mobs_killed": "events", "crafting": "events", "pickups": "events",
    "advancements": "events", "stat_awards": "stats",
}
failures = []


def check(name, ok, detail=""):
    print(f'{"ok  " if ok else "FAIL"} {name}' + (f": {detail}" if detail and not ok else ""), flush=True)
    if not ok:
        failures.append(name)


class Game:
    def __init__(self, bin_dir, resources):
        self.root = Path(tempfile.mkdtemp(prefix="dxp-")).resolve()
        for name in ("home", "launcher", "data", "config", "game"):
            (self.root / name).mkdir()
        self.game = self.root / "game"
        self.ctl_path = Path(bin_dir).resolve() / "horizon-ctl"
        self.socket = self.root / "c"
        self.env = dict(
            os.environ, HORIZON_HOME=str(self.root / "home"), HORIZON_LAUNCHER_HOME=str(self.root / "launcher"),
            XDG_DATA_HOME=str(self.root / "data"), XDG_CONFIG_HOME=str(self.root / "config"),
            HORIZON_LAUNCHER_NO_SELF_UPDATE="1", HORIZON_NO_AUDIO="1",
        )
        self.run("launch", "--world", "Proof", "--wait", "--offscreen", "--name", "Tester",
                 "--resources", str(Path(resources).resolve()), "--game-dir", str(self.game))

    def run(self, *words, tolerate=False):
        done = subprocess.run([str(self.ctl_path), "--socket", str(self.socket), *map(str, words)], env=self.env,
                              cwd=self.game, capture_output=True, text=True, timeout=120)
        reply = json.loads(done.stdout or "{}")
        if "error" in reply and not tolerate:
            raise RuntimeError(f"{' '.join(map(str, words))}: {reply['error']}")
        return reply

    def labels(self):
        return [w.get("label", "") for w in self.run("ui")["widgets"]]

    def press(self, start):
        for widget in self.run("ui")["widgets"]:
            if widget.get("label", "").startswith(start):
                return self.run("click", widget["id"])
        raise RuntimeError(f"no button starts with {start!r}: {self.labels()}")

    def page(self):
        self.run("options", "mods")
        time.sleep(0.5)
        self.press("Settings")
        time.sleep(0.5)

    def quit(self):
        self.run("quit", tolerate=True)
        time.sleep(3)

    @property
    def profile(self):
        return self.game / "config" / "data_exporter.json"

    def records(self, kind):
        folder = self.game / "data-exporter" / KINDS[kind] / kind
        out = []
        for path in sorted(folder.glob("*.jsonl")) if folder.exists() else []:
            out.extend(json.loads(line) for line in path.read_text().splitlines())
        return out


def beside(g):
    p = g.run("state")["player"]["block_pos"]
    return p[0] + 2, p[1], p[2]


def play(g, open_location):
    g.run("wait-ready", 60)
    g.run("options-set", "guiScale", 1, tolerate=True)  # every button on one screen
    check("the profile file is created", g.profile.exists())
    default = json.loads(g.profile.read_text())
    check("the profile starts in JSONL with the other players off",
          default["format"] == "jsonl" and default["include_other_players"] is False, default)

    g.page()
    labels = g.labels()
    for wanted in ("Session & player: ON", "Blocks & combat: ON", "Items & progress: ON", "Statistics: ON",
                   "Format: JSONL", "Open profile location", "Copy profile path"):
        check(f"button {wanted}", wanted in labels, labels)
    g.press("Blocks & combat")
    time.sleep(0.3)
    off = json.loads(g.profile.read_text())["capture"]
    check("a group button rewrites the profile", off["block_broken"] is False and off["mobs_killed"] is False, off)
    g.press("Blocks & combat")
    time.sleep(0.3)
    g.press("Copy profile path")
    time.sleep(0.3)
    status = g.run("status")
    check("the path reaches the clipboard", str(g.profile) in json.dumps(status), status.get("clipboard"))
    if open_location:
        g.press("Open profile location")
    g.run("close")
    time.sleep(0.5)

    g.run("command", "gamemode survival")
    x, y, z = beside(g)
    g.run("command", f"setblock {x} {y} {z} minecraft:dirt")
    time.sleep(0.5)
    print("break", json.dumps(g.run("break", x, y, z, "--timeout", 20))[:160], flush=True)
    time.sleep(1)
    g.run("command", f"tp @s {x}.5 {y} {z}.5")  # onto the drop
    time.sleep(2)
    g.run("give", "minecraft:dirt", 2)
    time.sleep(0.5)
    print("place", json.dumps(g.run("place", x, y, z, "--item", "minecraft:dirt", tolerate=True))[:160], flush=True)
    g.run("give", "minecraft:oak_log", 1)
    time.sleep(0.5)
    g.run("open-inventory")
    time.sleep(0.5)
    slots = g.run("screen").get("slots", [])
    log = next((s for s in slots if "oak_log" in json.dumps(s)), None)
    print("log slot", json.dumps(log)[:120], flush=True)
    if log is not None:
        g.run("click-slot", log.get("slot", log.get("index")))
        time.sleep(0.2)
        g.run("click-slot", 100)
        time.sleep(0.4)
        g.run("click-slot", 200, "shift")
        time.sleep(0.4)
    g.run("close")
    time.sleep(0.3)

    g.run("command", "advancement grant @s only minecraft:story/root")
    x, y, z = beside(g)
    print("summon", json.dumps(g.run("command", f"summon minecraft:pig {x} {y} {z}"))[:140], flush=True)
    time.sleep(1.5)
    near = g.run("entities", 8).get("entities", [])
    print("entities", json.dumps(near)[:200], flush=True)
    print("kill", json.dumps(g.run("command", "damage @e[type=minecraft:pig,limit=1] 100 minecraft:player_attack by @s", tolerate=True))[:160], flush=True)
    time.sleep(1.5)
    g.run("command", "execute in minecraft:the_nether run tp @s 0 120 0")
    time.sleep(2)
    g.run("command", "execute in minecraft:overworld run tp @s 0 120 0")
    time.sleep(7)

    # The profile edited by hand: the mod follows it, and a broken file changes nothing.
    edited = json.loads(g.profile.read_text())
    edited["format"] = "csv"
    g.profile.write_text(json.dumps(edited))
    time.sleep(2.5)
    g.run("command", "advancement grant @s only minecraft:story/mine_stone")
    time.sleep(2.5)
    csv_files = list((g.game / "data-exporter/events/advancements").glob("*.csv"))
    check("an edited profile applies while playing", len(csv_files) == 1, csv_files)
    if csv_files:
        rows = list(csv.DictReader(csv_files[0].open()))
        check("the CSV has a header and the advancement",
              rows and rows[0]["advancement"] == "minecraft:story/mine_stone", rows[:1])
    g.profile.write_text("{ broken")
    time.sleep(2.5)
    g.page()
    check("a broken profile is reported", "Status: the profile has an error" in g.labels(), g.labels())
    g.run("close")
    time.sleep(0.3)
    check("a broken profile is left for the player to fix", g.profile.read_text() == "{ broken")
    edited["format"] = "jsonl"
    g.profile.write_text(json.dumps(edited))
    time.sleep(2.5)

    g.run("command", "kill @s")
    time.sleep(2)
    g.run("click-button", "Respawn", tolerate=True)
    time.sleep(2)


def inspect(g):
    for kind in KINDS:
        records = g.records(kind)
        check(f"{kind} has records", bool(records), len(records))
    for record in g.records("block_broken")[:1]:
        check("a block keeps its coordinates and name",
              record["block"].startswith("minecraft:dirt") and isinstance(record["block_x"], int), record)
        check("records carry the schema and a UTC time", record["schema"] == 1 and record["ts"].endswith("Z"), record)
        check("records name the player", record["player_name"] == "Tester" and len(record["player_uuid"]) == 36, record)
    sessions = [r["action"] for r in g.records("session")]
    check("the session has its join", "join" in sessions, sessions)
    deaths = g.records("player_deaths")
    print("death", json.dumps(deaths[:1])[:200], flush=True)
    state = g.records("player_state")
    print("state", json.dumps(state[:1])[:320], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin", required=True)
    parser.add_argument("--resources", required=True)
    parser.add_argument("--open", action="store_true")
    args = parser.parse_args()
    g = Game(args.bin, args.resources)
    try:
        play(g, args.open)
    finally:
        g.quit()
    inspect(g)
    print("--- files")
    base = g.game / "data-exporter"
    for path in sorted(p for p in base.rglob("*") if p.is_file()):
        print(path.relative_to(base), path.stat().st_size)
    shutil.rmtree(g.root, ignore_errors=True)
    print("every check passed" if not failures else f"{len(failures)} failed: {failures}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
