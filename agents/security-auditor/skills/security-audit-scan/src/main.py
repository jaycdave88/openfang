#!/usr/bin/env python3
"""OpenFang skill: security-audit-scan
Scans git repos for malicious patterns, audits dependencies,
checks for data exfiltration, and generates PASS/FAIL/WARN reports."""

import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

# --- Default allowlist path ---
DEFAULT_ALLOWLIST = os.path.expanduser("~/.openfang/security-allowlist.toml")

# --- Malicious code patterns ---
MALICIOUS_PATTERNS = [
    (r'\beval\s*\(', "eval() call — potential code injection", "HIGH"),
    (r'\bexec\s*\(', "exec() call — potential code injection", "HIGH"),
    (r'subprocess\.\w+\(.*shell\s*=\s*True', "subprocess with shell=True — command injection risk", "CRITICAL"),
    (r'os\.system\s*\(', "os.system() — command injection risk", "HIGH"),
    (r'os\.popen\s*\(', "os.popen() — command injection risk", "HIGH"),
    (r'__import__\s*\(', "Dynamic import — potential code loading", "MEDIUM"),
    (r'compile\s*\(.*exec', "compile+exec — dynamic code execution", "HIGH"),
    (r'pickle\.loads?\s*\(', "pickle deserialization — arbitrary code execution", "CRITICAL"),
    (r'yaml\.load\s*\((?!.*Loader)', "yaml.load without SafeLoader — code execution", "HIGH"),
    (r'marshal\.loads?\s*\(', "marshal deserialization — code execution risk", "HIGH"),
    (r'\\x[0-9a-fA-F]{2}(?:\\x[0-9a-fA-F]{2}){10,}', "Long hex-encoded string — possible obfuscation", "MEDIUM"),
    (r'base64\.b64decode\s*\(.*(?:eval|exec|subprocess|os\.system)', "Base64 decode + execution — obfuscated payload", "CRITICAL"),
    (r'requests\.(?:get|post|put|delete|patch)\s*\(', "HTTP request — check against domain allowlist", "INFO"),
    (r'urllib\.request\.urlopen\s*\(', "URL fetch — check against domain allowlist", "INFO"),
    (r'httpx\.(?:get|post|put|delete|patch|Client)\s*\(', "HTTP request via httpx", "INFO"),
    (r'aiohttp\.ClientSession\s*\(', "Async HTTP client — check domain allowlist", "INFO"),
    (r'socket\.connect\s*\(', "Raw socket connection — potential data exfiltration", "HIGH"),
    (r'ctypes\.\w+', "ctypes usage — native code execution", "MEDIUM"),
    (r'webbrowser\.open\s*\(', "Browser open — potential phishing", "LOW"),
    (r'keyring\.\w+', "Keyring access — credential theft risk", "HIGH"),
    (r'getpass\.\w+', "Password input capture", "MEDIUM"),
]

SCAN_EXTENSIONS = {'.py', '.js', '.ts', '.jsx', '.tsx', '.sh', '.bash', '.rb', '.php', '.go', '.rs'}
SKIP_DIRS = {'.git', 'node_modules', '__pycache__', '.venv', 'venv', '.tox', 'dist', 'build', '.eggs', 'target'}


def load_allowlist(path):
    """Load allowlist/blocklist config (simple TOML parsing)."""
    config = {"allowed_domains": [], "blocked_domains": [], "allowed_commands": [], "blocked_commands": []}
    if not os.path.exists(path):
        return config
    try:
        # Simple TOML parser for flat arrays
        current_key = None
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("#") or not line:
                    continue
                if "=" in line and not line.startswith('"'):
                    key, val = line.split("=", 1)
                    key = key.strip()
                    if key in config:
                        # Parse TOML array
                        val = val.strip()
                        if val.startswith("["):
                            items = re.findall(r'"([^"]*)"', val)
                            config[key] = items
    except Exception:
        pass
    return config




def scan_code_patterns(repo_path, allowlist):
    """Scan source files for malicious patterns."""
    findings = []
    blocked = set(allowlist.get("blocked_domains", []))
    allowed = set(allowlist.get("allowed_domains", []))

    for root, dirs, files in os.walk(repo_path):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for fname in files:
            ext = os.path.splitext(fname)[1].lower()
            if ext not in SCAN_EXTENSIONS:
                continue
            fpath = os.path.join(root, fname)
            rel = os.path.relpath(fpath, repo_path)
            try:
                with open(fpath, errors="replace") as f:
                    for lineno, line in enumerate(f, 1):
                        for pattern, desc, severity in MALICIOUS_PATTERNS:
                            if re.search(pattern, line):
                                if severity == "INFO" and ("request" in desc.lower() or "fetch" in desc.lower() or "http" in desc.lower()):
                                    urls = re.findall(r'["\']https?://([^/"\':]+)', line)
                                    for domain in urls:
                                        if domain in blocked:
                                            findings.append({"file": rel, "line": lineno, "severity": "CRITICAL",
                                                             "finding": f"Blocked domain: {domain}", "evidence": line.strip()[:200]})
                                        elif domain not in allowed and not domain.startswith("localhost") and not domain.startswith("127."):
                                            findings.append({"file": rel, "line": lineno, "severity": "WARN",
                                                             "finding": f"Unknown domain: {domain} — not in allowlist", "evidence": line.strip()[:200]})
                                    continue
                                findings.append({"file": rel, "line": lineno, "severity": severity,
                                                 "finding": desc, "evidence": line.strip()[:200]})
            except (OSError, UnicodeDecodeError):
                continue
    return findings


def audit_python_deps(repo_path):
    """Run pip-audit on Python dependencies."""
    findings = []
    req_files = list(Path(repo_path).rglob("requirements*.txt"))
    setup_py = Path(repo_path) / "setup.py"
    pyproject = Path(repo_path) / "pyproject.toml"
    if not req_files and not setup_py.exists() and not pyproject.exists():
        return findings
    for req in req_files:
        try:
            result = subprocess.run(
                ["pip-audit", "-r", str(req), "--format", "json", "--progress-spinner", "off"],
                capture_output=True, text=True, timeout=120, cwd=repo_path)
            if result.stdout:
                vulns = json.loads(result.stdout)
                if isinstance(vulns, dict):
                    vulns = vulns.get("dependencies", [])
                for v in vulns:
                    if isinstance(v, dict) and v.get("vulns"):
                        for vuln in v["vulns"]:
                            findings.append({"file": str(req.relative_to(repo_path)), "severity": "HIGH",
                                "finding": f"Vulnerable dep: {v.get('name','?')} {v.get('version','?')} — {vuln.get('id','?')}",
                                "evidence": vuln.get("description", "")[:200]})
        except (subprocess.TimeoutExpired, FileNotFoundError, json.JSONDecodeError):
            findings.append({"file": str(req.relative_to(repo_path)), "severity": "INFO",
                             "finding": "pip-audit unavailable or timed out"})
    return findings


def audit_node_deps(repo_path):
    """Run npm audit on Node.js dependencies."""
    findings = []
    if not (Path(repo_path) / "package.json").exists():
        return findings
    try:
        result = subprocess.run(["npm", "audit", "--json"], capture_output=True, text=True, timeout=120, cwd=repo_path)
        if result.stdout:
            data = json.loads(result.stdout)
            for name, info in data.get("vulnerabilities", {}).items():
                sev = info.get("severity", "low").upper()
                if sev == "MODERATE":
                    sev = "MEDIUM"
                findings.append({"file": "package.json", "severity": sev if sev in ("CRITICAL","HIGH","MEDIUM","LOW") else "MEDIUM",
                    "finding": f"Vulnerable npm pkg: {name} — {info.get('title','?')}",
                    "evidence": f"Range: {info.get('range','?')}, Fix: {info.get('fixAvailable','?')}"})
    except (subprocess.TimeoutExpired, FileNotFoundError, json.JSONDecodeError):
        findings.append({"file": "package.json", "severity": "INFO", "finding": "npm audit unavailable or timed out"})
    return findings


def calculate_risk_score(findings):
    """Calculate risk score 0-100 based on findings."""
    weights = {"CRITICAL": 25, "HIGH": 15, "MEDIUM": 5, "WARN": 3, "LOW": 1, "INFO": 0}
    score = 0
    for f in findings:
        score += weights.get(f.get("severity", "INFO"), 0)
    return min(score, 100)


def generate_report(repo_path, findings, risk_score):
    """Generate a structured security report."""
    timestamp = datetime.now(timezone.utc).isoformat()
    repo_name = os.path.basename(os.path.normpath(repo_path))

    if risk_score > 70:
        verdict = "FAIL"
    elif risk_score >= 30:
        verdict = "WARN"
    else:
        verdict = "PASS"

    severity_counts = {}
    for f in findings:
        sev = f.get("severity", "INFO")
        severity_counts[sev] = severity_counts.get(sev, 0) + 1

    report = {
        "repo": repo_name,
        "repo_path": repo_path,
        "timestamp": timestamp,
        "verdict": verdict,
        "risk_score": risk_score,
        "total_findings": len(findings),
        "severity_counts": severity_counts,
        "findings": findings[:50],  # Cap at 50 for readability
        "summary": f"Security scan of {repo_name}: {verdict} (risk score: {risk_score}/100). "
                   f"Found {len(findings)} issue(s): "
                   + ", ".join(f"{v} {k}" for k, v in sorted(severity_counts.items(), key=lambda x: -x[1]))
    }
    return report


def security_scan(repo_path, check_deps=True, allowlist_path=None):
    """Main scan entry point."""
    if not os.path.isdir(repo_path):
        return {"error": f"Repository path does not exist: {repo_path}"}

    al_path = allowlist_path or DEFAULT_ALLOWLIST
    allowlist = load_allowlist(al_path)

    findings = []
    # 1. Code pattern scan
    findings.extend(scan_code_patterns(repo_path, allowlist))
    # 2. Dependency audits
    if check_deps:
        findings.extend(audit_python_deps(repo_path))
        findings.extend(audit_node_deps(repo_path))

    risk_score = calculate_risk_score(findings)
    report = generate_report(repo_path, findings, risk_score)
    return report


def main():
    payload = json.loads(sys.stdin.read())
    tool_name = payload.get("tool", "security_scan")
    input_data = payload.get("input", {})

    try:
        if tool_name == "security_scan":
            result = security_scan(
                repo_path=input_data["repo_path"],
                check_deps=input_data.get("check_deps", True),
                allowlist_path=input_data.get("allowlist_path"),
            )
            print(json.dumps({"result": json.dumps(result, indent=2)}))
        else:
            print(json.dumps({"error": f"Unknown tool: {tool_name}"}))
    except Exception as e:
        print(json.dumps({"error": str(e)}))


if __name__ == "__main__":
    main()