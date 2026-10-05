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

// The folder to reveal for an export result. Exports either write one file
// (e.g. "talk_podcast.m4a") or a directory named after the source stem
// (e.g. "talk_podcast_clips"), which never carries an extension. A path with an
// extension is therefore a file and its parent folder is returned; anything
// else is already the folder.
export function exportFolderOf(path: string): string {
    if (!/\.[^/\\.]+$/.test(path)) {
        return path;
    }
    const parent = path.replace(/[/\\][^/\\]+$/, '');
    return parent === path ? path : parent;
}
