// Inserts a suffix before a file's extension, e.g. appending "_cut" to
// "/videos/talk.mp4" yields "/videos/talk_cut.mp4". The extension is matched
// with the same character class used elsewhere for path handling (excluding
// path separators so a dot in a parent directory is never mistaken for an
// extension). Paths without an extension simply get the suffix appended.
export function appendFileNameSuffix(path: string, suffix: string): string {
  const match = path.match(/\.[^/\\.]+$/);
  if (!match) {
    return path + suffix;
  }
  const extension = match[0];
  return path.slice(0, path.length - extension.length) + suffix + extension;
}
