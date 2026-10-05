import register from '../assets/runtime-licenses.json';

export type RuntimeDownloadKind =
    'executable' | 'library' | 'model-weights' | 'python-package' | 'font';

export interface RuntimeDownload {
    id: string;
    name: string;
    kind: RuntimeDownloadKind;
    usedBy: string;
    source: string;
    license: string;
    licenseUrl: string;
    /** Shown in the app; required for licences that need attribution or an exception. */
    notice?: string;
}

export interface RuntimeLicenseException {
    license: string;
    appliesTo: string[];
    reason: string;
}

export interface RuntimeLicenseRegister {
    policy: {
        allowed: string[];
        requiresNotice: string[];
        exceptions: RuntimeLicenseException[];
    };
    downloads: RuntimeDownload[];
}

export const runtimeLicenses = register as RuntimeLicenseRegister;

export function runtimeDownload(id: string): RuntimeDownload {
    const download = runtimeLicenses.downloads.find((entry) => entry.id === id);
    if (!download) {
        throw new Error(`Unknown runtime download: ${id}`);
    }
    return download;
}

/** Policy violations of a register; empty when it complies. */
export function runtimeLicenseViolations(register: RuntimeLicenseRegister): string[] {
    const { allowed, requiresNotice, exceptions } = register.policy;
    const violations: string[] = [];

    for (const download of register.downloads) {
        const exception = exceptions.find((entry) => entry.license === download.license);
        if (exception) {
            if (!exception.appliesTo.includes(download.id)) {
                violations.push(
                    `${download.id}: ${download.license} is only excepted for ${exception.appliesTo.join(', ')}`,
                );
            }
        } else if (!allowed.includes(download.license)) {
            violations.push(`${download.id}: licence ${download.license} is not allowed`);
        }

        const needsNotice = !!exception || requiresNotice.includes(download.license);
        if (needsNotice && !download.notice?.trim()) {
            violations.push(`${download.id}: ${download.license} requires a notice`);
        }
    }

    for (const exception of exceptions) {
        if (!exception.reason.trim()) {
            violations.push(`exception for ${exception.license} has no reason`);
        }
        for (const id of exception.appliesTo) {
            if (!register.downloads.some((download) => download.id === id)) {
                violations.push(`exception for ${exception.license} names unknown download ${id}`);
            }
        }
    }

    return violations;
}
