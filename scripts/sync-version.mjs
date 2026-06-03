import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

const repoRoot = process.cwd();
const packageJsonPath = path.join(repoRoot, 'package.json');
const cargoTomlPath = path.join(repoRoot, 'Cargo.toml');
const changelogPath = path.join(repoRoot, 'CHANGELOG.md');

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
const nextVersion = packageJson.version;
const today = new Date().toISOString().slice(0, 10);

if (typeof nextVersion !== 'string' || nextVersion.length === 0) {
  throw new Error('package.json is missing a valid version');
}

const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
const updatedCargoToml = cargoToml.replace(
  /(^version\s*=\s*")([^"]+)(")/m,
  `$1${nextVersion}$3`,
);

if (updatedCargoToml !== cargoToml) {
  fs.writeFileSync(cargoTomlPath, updatedCargoToml);
}

updateChangelog(nextVersion, today);

try {
  execFileSync('git', ['rev-parse', '--is-inside-work-tree'], {
    cwd: repoRoot,
    stdio: 'ignore',
  });
  execFileSync('git', ['add', 'package.json', 'Cargo.toml', 'CHANGELOG.md'], {
    cwd: repoRoot,
    stdio: 'ignore',
  });
} catch {
  // Allow use outside git or with --no-git-tag-version.
}

console.log(`Synchronized version ${nextVersion} across package.json, Cargo.toml, and CHANGELOG.md`);

function updateChangelog(version, date) {
  const versionHeading = `## [${version}] - ${date}`;
  const changelog = fs.existsSync(changelogPath)
    ? fs.readFileSync(changelogPath, 'utf8')
    : defaultChangelog();

  if (new RegExp(`^## \\[${escapeRegExp(version)}\\](?: - .+)?$`, 'm').test(changelog)) {
    return;
  }

  const unreleasedHeading = '## [Unreleased]';
  const insertion = `${unreleasedHeading}\n\n${versionHeading}\n- Version bump.\n\n`;

  if (changelog.includes(`${unreleasedHeading}\n\n`)) {
    fs.writeFileSync(
      changelogPath,
      changelog.replace(`${unreleasedHeading}\n\n`, insertion),
    );
    return;
  }

  if (changelog.includes(unreleasedHeading)) {
    fs.writeFileSync(
      changelogPath,
      changelog.replace(unreleasedHeading, insertion.trimEnd()),
    );
    return;
  }

  const firstVersionMatch = changelog.match(/^## \[[^\]]+\](?: - .+)?$/m);
  if (firstVersionMatch && firstVersionMatch.index !== undefined) {
    const beforeVersions = changelog.slice(0, firstVersionMatch.index).trimEnd();
    const versions = changelog.slice(firstVersionMatch.index).trimStart();
    fs.writeFileSync(
      changelogPath,
      `${beforeVersions}\n\n${versionHeading}\n- Version bump.\n\n${versions}`,
    );
    return;
  }

  fs.writeFileSync(
    changelogPath,
    `${changelog.trimEnd()}\n\n${versionHeading}\n- Version bump.\n`,
  );
}

function defaultChangelog() {
  return [
    '# Changelog',
    '',
    'All notable changes to this project will be documented in this file.',
    '',
    '## [Unreleased]',
    '',
  ].join('\n');
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}