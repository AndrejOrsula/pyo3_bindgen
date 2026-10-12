#!/usr/bin/env python3
"""Build the pinned SeaBIOS, Linux, and BusyBox VM assets for the browser compiler."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zlib
from pathlib import Path
from typing import Any

COMPILER_DIR = Path(__file__).resolve().parent
PINS_PATH = COMPILER_DIR / "assets.json"
REPRO_EPOCH = 1_767_225_600  # 2026-01-01T00:00:00Z
KBUILD_TIMESTAMP = "Thu Jan  1 00:00:00 UTC 2026"
SEABIOS_VERSION = "rel-1.16.2-0-gea1b7a0"
SEABIOS_EXTRAVERSION = "-pyo3-browser-v1"
BUILD_OVERRIDE_NAMES = frozenset({
    "ARCH", "AS", "ASFLAGS", "AR", "CC", "CFLAGS", "CLANG_TRIPLE", "CPP",
    "CPPFLAGS", "CROSS_COMPILE", "CROSS_COMPILE_ARM32", "CROSS_PREFIX", "CXX",
    "CXXFLAGS", "GNUMAKEFLAGS", "HOSTCC", "HOSTCFLAGS", "HOSTCXX",
    "HOSTLD", "HOSTAR", "HOSTRANLIB", "HOSTLDFLAGS", "CFLAGS_KERNEL",
    "CFLAGS_MODULE", "LDFLAGS_MODULE", "LDFLAGS_vmlinux", "CFLAGS_FOR_BUILD",
    "CXXFLAGS_FOR_BUILD", "CPPFLAGS_FOR_BUILD", "LDFLAGS_FOR_BUILD",
    "CFLAGS_FOR_TARGET", "CXXFLAGS_FOR_TARGET", "CPPFLAGS_FOR_TARGET",
    "LDFLAGS_FOR_TARGET", "KBUILD_EXTMOD", "KBUILD_EXTRA_SYMBOLS", "KBUILD_MODPOST_NOFINAL",
    "KBUILD_OUTPUT", "KBUILD_VERBOSE", "KCFLAGS", "KCPPFLAGS", "KAFLAGS", "LD",
    "LDFLAGS", "MAKE", "MAKEFILES", "MAKEFLAGS", "MAKELEVEL", "MAKEOVERRIDES",
    "MAKECMDGOALS", "MFLAGS", "NM", "OBJCOPY", "OBJDUMP", "RANLIB", "READELF",
    "O", "STRIP", "SUBARCH", "BISON", "FLEX", "YACC", "LEX", "PERL", "PYTHON",
    "GCC_EXEC_PREFIX", "COMPILER_PATH", "LIBRARY_PATH", "CPATH", "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH", "DEPENDENCIES_OUTPUT",
    "SUNPRO_DEPENDENCIES", "LLVM", "LLVM_IAS", "LLVM_PREFIX", "LLVM_SUFFIX",
    "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR",
    "PYTHONHOME", "PYTHONPATH", "PYTHONSTARTUP", "PYTHONINSPECT", "PERL5LIB",
    "PERL5OPT", "TAR_OPTIONS", "XZ_OPT", "BZIP2", "GZIP",
})
BUILD_OVERRIDE_PREFIXES = ("KBUILD_", "KCONFIG_", "CONFIG_")

# Build-only host tools are task-local, hash-checked Ubuntu packages. Their .deb
# copyright files remain available in the cache; none of these tools ship in VM.
TOOL_PINS: dict[str, dict[str, str | int]] = {
    "acpica-tools": {
        "file": "acpica-tools_20200925-6_amd64.deb",
        "url": "https://archive.ubuntu.com/ubuntu/pool/universe/a/acpica-unix/acpica-tools_20200925-6_amd64.deb",
        "sha256": "3a7ad820f8c35d392176f91a1e293e346f45df5cb516f2e551f87915bc3c7d08",
        "bytes": 899738,
        "version": "20200925-6",
    },
    "bison": {
        "file": "bison_3.8.2+dfsg-1ubuntu0.22.04.1_amd64.deb",
        "url": "https://archive.ubuntu.com/ubuntu/pool/main/b/bison/bison_3.8.2+dfsg-1ubuntu0.22.04.1_amd64.deb",
        "sha256": "40ba998a24ce53a420fc3f8188b833fde276564df1a7e90acaf875f4f9b76c4e",
        "bytes": 748182,
        "version": "2:3.8.2+dfsg-1ubuntu0.22.04.1",
    },
    "flex": {
        "file": "flex_2.6.4-8build2_amd64.deb",
        "url": "https://archive.ubuntu.com/ubuntu/pool/main/f/flex/flex_2.6.4-8build2_amd64.deb",
        "sha256": "980674f410fa6cb5e41d1bdf434a9391dcec0ac8fae5194cd90f9598ae074d8d",
        "bytes": 306728,
        "version": "2.6.4-8build2",
    },
    "libsigsegv2": {
        "file": "libsigsegv2_2.13-1ubuntu3_amd64.deb",
        "url": "https://archive.ubuntu.com/ubuntu/pool/main/libs/libsigsegv/libsigsegv2_2.13-1ubuntu3_amd64.deb",
        "sha256": "894fcb0804df54dca59edbd0860d6f3eeac25d1555aebd3fa99d80665969f891",
        "bytes": 14612,
        "version": "2.13-1ubuntu3",
    },
    "m4": {
        "file": "m4_1.4.18-5ubuntu2_amd64.deb",
        "url": "https://archive.ubuntu.com/ubuntu/pool/main/m/m4/m4_1.4.18-5ubuntu2_amd64.deb",
        "sha256": "572a544d2c18bf49d25c465720c570cd8e6e38731386ac9c0a7f29bed2486f3e",
        "bytes": 198836,
        "version": "1.4.18-5ubuntu2",
    },
}
ZIG_PIN = {
    "file": "zig-linux-x86_64-0.13.0.tar.xz",
    "url": "https://ziglang.org/download/0.13.0/zig-linux-x86_64-0.13.0.tar.xz",
    "sha256": "d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea",
    "bytes": 47082308,
    "version": "0.13.0",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_file(path: Path, pin: dict[str, Any], label: str) -> dict[str, Any]:
    if not path.is_file():
        raise RuntimeError(f"missing {label}: {path}")
    actual_bytes = path.stat().st_size
    actual_hash = sha256(path)
    if actual_bytes != int(pin["bytes"]) or actual_hash != pin["sha256"]:
        raise RuntimeError(
            f"{label} pin mismatch: expected {pin['bytes']} bytes/{pin['sha256']}, "
            f"got {actual_bytes}/{actual_hash}: {path}"
        )
    return {"file": str(pin["file"]), "sha256": actual_hash, "bytes": actual_bytes}


def download_pin(cache: Path, pin: dict[str, Any], label: str) -> Path:
    path = cache / str(pin["file"])
    if path.exists():
        try:
            verify_file(path, pin, label)
            return path
        except RuntimeError:
            path.unlink()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".partial")
    temporary.unlink(missing_ok=True)
    request = urllib.request.Request(str(pin["url"]), headers={"User-Agent": "pyo3-bindgen-vm-build/1"})
    expected_bytes = int(pin["bytes"])
    if expected_bytes < 0:
        raise RuntimeError(f"{label} pin has a negative size")
    try:
        with urllib.request.urlopen(request, timeout=60) as response, temporary.open("wb") as target:
            content_length = response.headers.get("Content-Length")
            if content_length is not None:
                try:
                    content_bytes = int(content_length)
                except ValueError as error:
                    raise RuntimeError(f"{label} response has invalid Content-Length") from error
                if content_bytes > expected_bytes:
                    raise RuntimeError(
                        f"{label} response exceeds pinned size: expected at most {expected_bytes} bytes, "
                        f"Content-Length is {content_length}"
                    )
            digest = hashlib.sha256()
            received = 0
            while True:
                chunk = response.read(min(1024 * 1024, expected_bytes - received + 1))
                if not chunk:
                    break
                if len(chunk) > expected_bytes - received:
                    raise RuntimeError(
                        f"{label} response exceeds pinned size: expected {expected_bytes} bytes"
                    )
                target.write(chunk)
                digest.update(chunk)
                received += len(chunk)
            if received != expected_bytes:
                raise RuntimeError(
                    f"{label} response size mismatch: expected {expected_bytes} bytes, got {received}"
                )
            if digest.hexdigest() != pin["sha256"]:
                raise RuntimeError(f"{label} response SHA-256 mismatch")
        verify_file(temporary, pin, label)
        temporary.replace(path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    return path


def find_local_pin(pins: dict[str, Any], file_name: str) -> dict[str, Any]:
    matches = [item for item in pins.get("sources", []) if item.get("file") == file_name and item.get("local")]
    if len(matches) != 1:
        raise RuntimeError(f"expected one local assets.json pin for {file_name}, found {len(matches)}")
    return matches[0]


def local_input(file_name: str, pins: dict[str, Any]) -> tuple[Path, dict[str, Any]]:
    pin = find_local_pin(pins, file_name)
    playground_dir = COMPILER_DIR.parent
    path = (playground_dir / str(pin["local"])).resolve()
    if playground_dir not in path.parents:
        raise RuntimeError(f"local input escapes playground directory: {pin['local']}")
    verify_file(path, pin, file_name)
    return path, pin


def source_input(name: str, pins: dict[str, Any], cache: Path) -> tuple[Path, dict[str, Any]]:
    pin = pins["inputs"][name]
    path = download_pin(cache, pin, name)
    verify_file(path, pin, name)
    return path, pin


def command_version(command: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(command, env=env, check=False, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if result.returncode:
        raise RuntimeError(f"version probe failed ({result.returncode}): {' '.join(command)}\n{result.stdout[-3000:]}")
    return result.stdout.strip().splitlines()[0] if result.stdout.strip() else ""


def require_command(name: str) -> str:
    found = shutil.which(name)
    if found is None:
        raise RuntimeError(f"host prerequisite missing: {name}")
    return found


def sanitized_build_environment(source: dict[str, str]) -> dict[str, str]:
    env = dict(source)
    env.pop("LD_LIBRARY_PATH", None)
    for name in BUILD_OVERRIDE_NAMES:
        env.pop(name, None)
    for name in tuple(env):
        if name.startswith(BUILD_OVERRIDE_PREFIXES):
            env.pop(name)
    return env


def host_tools(base_env: dict[str, str]) -> dict[str, str]:
    required = [
        "gcc", "ld", "make", "as", "ar", "nm", "objcopy", "objdump", "strip", "cpp",
        "perl", "bc", "tar", "xz", "bzip2", "gzip", "dpkg-deb", "readelf",
    ]
    paths = {name: require_command(name) for name in required}
    versions = {
        "gcc": command_version([paths["gcc"], "--version"], base_env),
        "ld": command_version([paths["ld"], "--version"], base_env),
        "make": command_version([paths["make"], "--version"], base_env),
        "python3": sys.version.split()[0],
    }
    gcc_match = re.search(r"\b(\d+)\.(\d+)", versions["gcc"])
    ld_match = re.search(r"\b(\d+)\.(\d+)", versions["ld"])
    make_match = re.search(r"\b(\d+)\.(\d+)", versions["make"])
    if not gcc_match or int(gcc_match.group(1)) < 11:
        raise RuntimeError(f"host GCC >= 11 required, found {versions['gcc']}")
    if not ld_match or tuple(map(int, ld_match.groups())) < (2, 38):
        raise RuntimeError(f"host GNU ld >= 2.38 required, found {versions['ld']}")
    if not make_match or tuple(map(int, make_match.groups())) < (4, 3):
        raise RuntimeError(f"host GNU make >= 4.3 required, found {versions['make']}")
    return {"paths": paths, "versions": versions}


def run_checked(
    args: list[str], cwd: Path, env: dict[str, str], label: str,
    log_dir: Path, stdin: bytes | None = None,
) -> str:
    result = subprocess.run(
        args, cwd=cwd, env=env, input=stdin, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, check=False,
    )
    output = result.stdout.decode("utf-8", errors="replace")
    (log_dir / f"{label}.log").write_text(output, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(f"{label} failed with exit {result.returncode}: {' '.join(args)}\n{output[-6000:]}")
    return output


def unpack(archive: Path, destination: Path, mode: str, env: dict[str, str], log_dir: Path, label: str) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    args = ["tar", mode, str(archive), "-C", str(destination), "--no-same-owner"]
    run_checked(args, destination, env, label, log_dir)
    entries = list(destination.iterdir())
    if len(entries) == 1 and entries[0].is_dir():
        return entries[0]
    if (destination / "Makefile").is_file():
        return destination
    raise RuntimeError(f"expected one source root in {archive.name}, found {[item.name for item in entries]}")


def unpack_tools(cache: Path, scratch: Path, base_env: dict[str, str]) -> tuple[Path, dict[str, Any]]:
    tool_cache = cache / "vm-tools"
    deb_dir = tool_cache / "debs"
    deb_dir.mkdir(parents=True, exist_ok=True)
    extract_root = scratch / "build-tools"
    extracted: dict[str, Path] = {}
    pins_receipt: dict[str, Any] = {}
    for name, pin in TOOL_PINS.items():
        package = download_pin(deb_dir, pin, name)
        verify_file(package, pin, name)
        dest = extract_root / name
        dest.mkdir(parents=True)
        subprocess.run(["dpkg-deb", "-x", str(package), str(dest)], env=base_env, check=True)
        extracted[name] = dest
        pins_receipt[name] = {"version": pin["version"], "file": pin["file"], "sha256": pin["sha256"], "bytes": pin["bytes"], "url": pin["url"]}
    zig_archive = download_pin(deb_dir, ZIG_PIN, "Zig")
    zig_root = unpack(zig_archive, extract_root / "zig", "-xJf", base_env, scratch, "extract-zig")
    zig_bin = zig_root / "zig"
    if not zig_bin.is_file():
        raise RuntimeError(f"pinned Zig archive lacks executable: {zig_bin}")
    zig_ver = subprocess.run([str(zig_bin), "version"], env=base_env, check=True, capture_output=True, text=True).stdout.strip()
    if zig_ver != str(ZIG_PIN["version"]):
        raise RuntimeError(f"Zig version pin mismatch: {zig_ver}")
    pins_receipt["zig"] = {"version": zig_ver, "file": ZIG_PIN["file"], "sha256": ZIG_PIN["sha256"], "bytes": ZIG_PIN["bytes"], "url": ZIG_PIN["url"]}

    tool_bin = scratch / "tool-bin"
    tool_bin.mkdir()
    binaries = {
        "bison": extracted["bison"] / "usr/bin/bison",
        "flex": extracted["flex"] / "usr/bin/flex",
        "m4": extracted["m4"] / "usr/bin/m4",
        "iasl": extracted["acpica-tools"] / "usr/bin/iasl",
    }
    for name, source in binaries.items():
        if not source.is_file():
            raise RuntimeError(f"pinned {name} package lacks {source}")
        (tool_bin / name).symlink_to(source)
    (tool_bin / "python").symlink_to(Path(sys.executable))
    env_paths = [str(tool_bin), base_env.get("PATH", os.defpath)]
    build_env = dict(base_env)
    build_env["PATH"] = os.pathsep.join(env_paths)
    build_env["BISON_PKGDATADIR"] = str(extracted["bison"] / "usr/share/bison")
    build_env["M4"] = str(tool_bin / "m4")
    build_env["IASL"] = str(tool_bin / "iasl")
    build_env["ZIG_CC_ZIG"] = str(zig_bin)
    libs = [str(extracted["libsigsegv2"] / "usr/lib/x86_64-linux-gnu")]
    build_env["LD_LIBRARY_PATH"] = os.pathsep.join(libs)
    for name, command in {
        "bison": [str(tool_bin / "bison"), "--version"],
        "flex": [str(tool_bin / "flex"), "--version"],
        "m4": [str(tool_bin / "m4"), "--version"],
        "iasl": [str(tool_bin / "iasl"), "-v"],
    }.items():
        pins_receipt[name + "_tool_version"] = command_version(command, build_env)
    return tool_bin, {"receipt": pins_receipt, "env": build_env, "zig_bin": zig_bin}


def write_newc(root: Path, destination: Path, epoch: int) -> None:
    def align4(buffer: bytearray) -> None:
        buffer.extend(b"\0" * (-len(buffer) % 4))

    names = ["."] + [path.relative_to(root).as_posix() for path in root.rglob("*")]
    names[1:] = sorted(names[1:], key=os.fsencode)
    archive = bytearray()
    for inode, name in enumerate(names, start=1):
        path = root if name == "." else root / name
        info = path.lstat()
        if stat.S_ISDIR(info.st_mode):
            mode = stat.S_IFDIR | stat.S_IMODE(info.st_mode)
            data = b""
            links = 2
        elif stat.S_ISREG(info.st_mode):
            mode = stat.S_IFREG | stat.S_IMODE(info.st_mode)
            data = path.read_bytes()
            links = 1
        elif stat.S_ISLNK(info.st_mode):
            mode = stat.S_IFLNK | stat.S_IMODE(info.st_mode)
            data = os.fsencode(os.readlink(path))
            links = 1
        else:
            raise RuntimeError(f"unsupported initramfs entry type: {path}")
        encoded_name = os.fsencode(name) + b"\0"
        fields = (
            inode, mode, 0, 0, links, epoch, len(data),
            0, 0, 0, 0, len(encoded_name), 0,
        )
        archive.extend(b"070701")
        for value in fields:
            archive.extend(f"{value:08x}".encode("ascii"))
        archive.extend(encoded_name)
        align4(archive)
        archive.extend(data)
        align4(archive)
    trailer = b"TRAILER!!!\0"
    archive.extend(b"070701")
    for value in (len(names) + 1, 0, 0, 0, 1, epoch, 0, 0, 0, 0, 0, len(trailer), 0):
        archive.extend(f"{value:08x}".encode("ascii"))
    archive.extend(trailer)
    align4(archive)
    with destination.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0) as compressed:
            compressed.write(archive)


def set_fixed_mtime(root: Path, epoch: int) -> None:
    items = [root, *root.rglob("*")]
    for path in items:
        os.utime(path, (epoch, epoch), follow_symlinks=False)


def clean_build_env(tool_info: dict[str, Any], scratch: Path) -> dict[str, str]:
    env = sanitized_build_environment(tool_info["env"])
    for name in ("LD_PRELOAD", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "DISPLAY"):
        env.pop(name, None)
    env.update({
        "RUSTUP_AUTO_INSTALL": "0",
        "SOURCE_DATE_EPOCH": str(REPRO_EPOCH),
        "KBUILD_BUILD_TIMESTAMP": KBUILD_TIMESTAMP,
        "KBUILD_BUILD_USER": "pyo3-vm-build",
        "KBUILD_BUILD_HOST": "reproducible-build",
        "KBUILD_BUILD_VERSION": "1",
        "KCONFIG_NOTIMESTAMP": "1",
        "TZ": "UTC",
        "LC_ALL": "C",
        "ZIG_GLOBAL_CACHE_DIR": str(scratch / "zig-global-cache"),
        "ZIG_LOCAL_CACHE_DIR": str(scratch / "zig-local-cache"),
    })
    return env


def build(args: argparse.Namespace) -> dict[str, Any]:
    pins = json.loads(PINS_PATH.read_text(encoding="utf-8"))
    if pins.get("schema_version") != 1:
        raise RuntimeError("unsupported compiler assets pin schema")
    cache = args.cache_dir.resolve()
    output = args.output_dir.resolve()
    if output == cache or output in cache.parents or cache in output.parents:
        raise RuntimeError("--cache-dir and --output-dir must not overlap")
    cache.mkdir(parents=True, exist_ok=True)
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output directory must be empty: {output}")
    output.mkdir(parents=True, exist_ok=True)

    source_inputs: dict[str, tuple[Path, dict[str, Any]]] = {
        name: source_input(name, pins, cache) for name in ("linux", "busybox", "seabios")
    }
    local_names = {
        "kernel_config": "linux-6.8.12.config",
        "busybox_config": "busybox-1.36.1.config",
        "init": "init",
        "seabios_config": "seabios-default.config",
    }
    local_inputs = {name: local_input(file_name, pins) for name, file_name in local_names.items()}
    base_env = sanitized_build_environment(os.environ.copy())
    for name in ("LD_PRELOAD", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "DISPLAY"):
        base_env.pop(name, None)
    base_env["RUSTUP_AUTO_INSTALL"] = "0"
    host = host_tools(base_env)

    with tempfile.TemporaryDirectory(prefix="pyo3-vm-build-") as temporary:
        scratch = Path(temporary)
        log_dir = scratch / "logs"
        log_dir.mkdir()
        _tool_bin, tool_info = unpack_tools(cache, scratch, base_env)
        env = clean_build_env(tool_info, scratch)
        jobs = min(16, max(1, os.cpu_count() or 1))
        host_paths = host["paths"]

        bios_dir = scratch / "seabios"
        bios_source = unpack(source_inputs["seabios"][0], bios_dir, "-xzf", env, log_dir, "extract-seabios")
        shutil.copyfile(local_inputs["seabios_config"][0], bios_source / ".config")
        (bios_source / ".version").write_text(SEABIOS_VERSION + "\n", encoding="ascii")
        (scratch / "tool-bin" / "python").unlink(missing_ok=True)
        (scratch / "tool-bin" / "python").symlink_to(Path(sys.executable))
        run_checked([host_paths["make"], "olddefconfig"], bios_source, env, "seabios-config", log_dir)
        run_checked(
            [host_paths["make"], f"EXTRAVERSION={SEABIOS_EXTRAVERSION}", f"-j{jobs}"],
            bios_source, env, "seabios-build", log_dir,
        )
        bios = bios_source / "out" / "bios.bin"
        if not bios.is_file() or bios.stat().st_size != 262144:
            raise RuntimeError("SeaBIOS output missing or not the expected 256 KiB image")
        bios_bytes = bios.read_bytes()
        if (SEABIOS_VERSION + SEABIOS_EXTRAVERSION).encode("ascii") not in bios_bytes:
            raise RuntimeError("SeaBIOS output lacks the pinned deterministic build version")
        if re.search(rb"\?-\d{8}_\d{6}-", bios_bytes):
            raise RuntimeError("SeaBIOS output embeds a nondeterministic timestamp")

        kernel_dir = scratch / "linux"
        kernel_source = unpack(source_inputs["linux"][0], kernel_dir, "-xJf", env, log_dir, "extract-linux")
        shutil.copyfile(local_inputs["kernel_config"][0], kernel_source / ".config")
        kernel_config_hash = sha256(kernel_source / ".config")
        run_checked([host_paths["make"], "ARCH=i386", "olddefconfig"], kernel_source, env, "kernel-config", log_dir)
        effective_kernel_config_hash = sha256(kernel_source / ".config")
        run_checked([host_paths["make"], "-s", "ARCH=i386", f"-j{jobs}", "bzImage"], kernel_source, env, "kernel-build", log_dir)
        kernel = kernel_source / "arch/x86/boot/bzImage"
        if not kernel.is_file():
            raise RuntimeError("Linux build did not produce arch/x86/boot/bzImage")

        busybox_dir = scratch / "busybox"
        busybox_source = unpack(source_inputs["busybox"][0], busybox_dir, "-xjf", env, log_dir, "extract-busybox")
        shutil.copyfile(local_inputs["busybox_config"][0], busybox_source / ".config")
        busybox_config_hash = sha256(busybox_source / ".config")
        run_checked(
            [host_paths["make"], "ARCH=i386", "oldconfig"], busybox_source, env,
            "busybox-config", log_dir, stdin=b"\n" * 2048,
        )
        effective_busybox_config_hash = sha256(busybox_source / ".config")
        wrapper = COMPILER_DIR / "provenance" / "zig-cc"
        run_checked(
            [host_paths["make"], "ARCH=i386", f"CC={wrapper}", f"-j{jobs}"],
            busybox_source, env, "busybox-build", log_dir,
        )
        busybox = busybox_source / "busybox"
        if not busybox.is_file():
            raise RuntimeError("BusyBox build did not produce busybox")
        elf = run_checked(["readelf", "-h", "-l", str(busybox)], busybox_source, env, "busybox-elf", log_dir)
        if "ELF32" not in elf or "Intel 80386" not in elf or "INTERP" in elf:
            raise RuntimeError("BusyBox must be a static ELF32 i386 binary")
        rootfs = scratch / "rootfs"
        run_checked(
            [host_paths["make"], "ARCH=i386", f"CC={wrapper}", f"CONFIG_PREFIX={rootfs}", "install"],
            busybox_source, env, "busybox-install", log_dir,
        )
        installed_busybox = rootfs / "bin" / "busybox"
        if not installed_busybox.is_file() or sha256(installed_busybox) != sha256(busybox):
            raise RuntimeError("installed BusyBox differs from the verified build output")
        installed_elf = run_checked(
            ["readelf", "-h", "-l", str(installed_busybox)], busybox_source, env,
            "busybox-installed-elf", log_dir,
        )
        if "ELF32" not in installed_elf or "Intel 80386" not in installed_elf or "INTERP" in installed_elf:
            raise RuntimeError("installed BusyBox must be a static ELF32 i386 binary")
        for directory in ("dev", "proc", "sys", "tmp", "mnt"):
            (rootfs / directory).mkdir(parents=True, exist_ok=True)
        init = rootfs / "init"
        shutil.copyfile(local_inputs["init"][0], init)
        init.chmod(0o755)
        for applet in ("sh", "tar", "gzip", "cp", "chmod", "rm", "cat", "mount", "mkdir"):
            candidate = rootfs / "bin" / applet
            if not candidate.is_symlink() or os.readlink(candidate) != "busybox":
                raise RuntimeError(f"required BusyBox applet is not installed: {applet}")
        set_fixed_mtime(rootfs, REPRO_EPOCH)
        initrd_path = scratch / "initramfs.cpio.gz"
        write_newc(rootfs, initrd_path, REPRO_EPOCH)

        result_paths = {
            "seabios-built.bin": bios,
            "bzImage": kernel,
            "initramfs.cpio.gz": initrd_path,
        }
        outputs: dict[str, dict[str, Any]] = {}
        for file_name, path in result_paths.items():
            target = output / file_name
            shutil.copyfile(path, target)
            target.chmod(0o644)
            outputs[file_name] = {"file": file_name, "sha256": sha256(target), "bytes": target.stat().st_size}

        build_receipt = {
            "schema_version": 1,
            "reproducible_metadata": {
                "source_date_epoch": REPRO_EPOCH,
                "kbuild_timestamp": KBUILD_TIMESTAMP,
                "kbuild_user": "pyo3-vm-build",
                "kbuild_host": "reproducible-build",
                "kbuild_version": "1",
                "seabios_version": SEABIOS_VERSION + SEABIOS_EXTRAVERSION,
                "initramfs_format": "newc",
                "initramfs_compression": "gzip -n -9",
                "initramfs_uid_gid": "0:0",
            },
            "inputs": {
                **{name: verify_file(path, pin, name) for name, (path, pin) in source_inputs.items()},
                **{name: verify_file(path, pin, name) for name, (path, pin) in local_inputs.items()},
            },
            "recipe": {
                "prepare_vm_sha256": sha256(Path(__file__).resolve()),
                "zig_cc_sha256": sha256(wrapper),
            },
            "toolchain": {
                "host": host["versions"],
                "task_local": tool_info["receipt"],
                "kernel_config_after_olddefconfig_sha256": effective_kernel_config_hash,
                "busybox_config_after_oldconfig_sha256": effective_busybox_config_hash,
                "kernel_config_before_olddefconfig_sha256": kernel_config_hash,
                "busybox_config_before_oldconfig_sha256": busybox_config_hash,
            },
            "commands": [
                "SeaBIOS: pin .version=rel-1.16.2-0-gea1b7a0 && make olddefconfig && make EXTRAVERSION=-pyo3-browser-v1 -jN",
                "Linux: make ARCH=i386 olddefconfig && make -s ARCH=i386 -jN bzImage",
                "BusyBox: make ARCH=i386 oldconfig && make ARCH=i386 CC=provenance/zig-cc -jN && make ARCH=i386 CC=provenance/zig-cc CONFIG_PREFIX=<rootfs> install",
                "initramfs: sorted newc entries (uid/gid 0, fixed epoch) then gzip -n -9",
            ],
            "outputs": outputs,
        }
        receipt_path = output / "vm-build.json"
        receipt_path.write_text(json.dumps(build_receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return build_receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, required=True, help="directory containing assets.json pinned source archives")
    parser.add_argument("--output-dir", type=Path, required=True, help="empty destination for VM binaries and vm-build.json")
    args = parser.parse_args()
    try:
        receipt = build(args)
    except (OSError, RuntimeError, subprocess.CalledProcessError, tarfile.TarError, json.JSONDecodeError, urllib.error.URLError) as error:
        print(f"prepare-vm.py: {error}", file=sys.stderr)
        return 1
    for name, item in receipt["outputs"].items():
        print(f"{name}: {item['bytes']} bytes sha256={item['sha256']}")
    print(f"receipt: {args.output_dir.resolve() / 'vm-build.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
