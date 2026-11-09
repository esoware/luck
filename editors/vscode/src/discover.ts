import * as vscode from "vscode";
import path from "node:path";
import * as fs from "node:fs";
import { logExtension } from "./output";
import { setting } from "./settings";

const BINARY_NAME = process.platform === "win32" ? "luck.exe" : "luck";

const VSIX_OPERATING_SYSTEMS: ReadonlySet<NodeJS.Platform> = new Set(["win32", "darwin", "linux"]);

const VSIX_PLATFORM = VSIX_OPERATING_SYSTEMS.has(process.platform)
	? `${process.platform}-${process.arch === "arm64" ? "arm64" : "x64"}`
	: null;

const WINDOWS_EXECUTABLE_EXTENSIONS = new Set(
	(process.env["PATHEXT"] ?? ".COM;.EXE;.BAT;.CMD")
		.split(";")
		.map((extension) => extension.trim().toLowerCase())
		.filter((extension) => extension.length > 0),
);

export async function discoverServer(context: vscode.ExtensionContext): Promise<string | null> {
	const explicitPath = setting("server.path").trim();
	if (explicitPath) {
		if (await isExecutable(explicitPath)) {
			logExtension(`server: using explicit path ${explicitPath}`);
			return explicitPath;
		}
		void vscode.window.showWarningMessage(
			`luck.server.path points at ${explicitPath} but no executable was found there.`,
		);
	}

	const bundledPath =
		VSIX_PLATFORM && path.join(context.extensionPath, "server", VSIX_PLATFORM, BINARY_NAME);
	if (bundledPath && (await isExecutable(bundledPath))) {
		logExtension(`server: using bundled binary ${bundledPath}`);
		if (!setting("runFromTemporaryLocation")) {
			return bundledPath;
		}
		const storagePath = context.globalStorageUri.fsPath;
		const copyPath = path.join(storagePath, BINARY_NAME);
		try {
			const bundled = await fs.promises.stat(bundledPath);
			const existing = await fs.promises.stat(copyPath).catch(() => null);
			if (existing?.size === bundled.size && existing.mtime.getTime() === bundled.mtime.getTime()) {
				logExtension(`server: reusing copy ${copyPath}`);
				return copyPath;
			}
			await fs.promises.mkdir(storagePath, { recursive: true });
			await fs.promises.copyFile(bundledPath, copyPath);
			if (process.platform !== "win32") {
				await fs.promises.chmod(copyPath, 0o755);
			}
			await fs.promises.utimes(copyPath, bundled.atime, bundled.mtime);
			logExtension(`server: copied to ${copyPath}`);
			return copyPath;
		} catch (error) {
			logExtension(`server: copy failed: ${error}`);
			return bundledPath;
		}
	}

	for (const directory of (process.env["PATH"] ?? "").split(path.delimiter)) {
		if (!directory) {
			continue;
		}
		const candidate = path.join(directory, BINARY_NAME);
		// oxlint-disable-next-line no-await-in-loop -- PATH order is a preference order; probe sequentially.
		if (await isExecutable(candidate)) {
			logExtension(`server: using PATH binary ${candidate}`);
			return candidate;
		}
	}

	logExtension("server: no binary found");
	return null;
}

async function isExecutable(filePath: string): Promise<boolean> {
	try {
		if (process.platform === "win32") {
			const stats = await fs.promises.stat(filePath);
			return (
				stats.isFile() && WINDOWS_EXECUTABLE_EXTENSIONS.has(path.extname(filePath).toLowerCase())
			);
		}
		await fs.promises.access(filePath, fs.constants.X_OK);
		return true;
	} catch {
		return false;
	}
}
