import * as vscode from "vscode";
import path from "node:path";
import * as fs from "node:fs";
import * as os from "node:os";
import { logExtension } from "./output";
import { setting } from "./settings";

const BINARY_NAME = process.platform === "win32" ? "luck.exe" : "luck";

export async function discoverServer(extensionPath: string): Promise<string | null> {
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

	const platform = vsixPlatform();
	const bundledPath = platform && path.join(extensionPath, "server", platform, BINARY_NAME);
	if (bundledPath && (await isExecutable(bundledPath))) {
		logExtension(`server: using bundled binary ${bundledPath}`);
		return setting("runFromTemporaryLocation") ? copyToTemporaryLocation(bundledPath) : bundledPath;
	}

	const binaryOnPath = await findOnPath();
	if (binaryOnPath) {
		logExtension(`server: using PATH binary ${binaryOnPath}`);
		return binaryOnPath;
	}

	logExtension("server: no binary found");
	return null;
}

async function copyToTemporaryLocation(source: string): Promise<string> {
	try {
		const directory = await fs.promises.mkdtemp(path.join(os.tmpdir(), "luck-"));
		const destination = path.join(directory, BINARY_NAME);
		await fs.promises.copyFile(source, destination);
		if (process.platform !== "win32") {
			await fs.promises.chmod(destination, 0o755);
		}
		logExtension(`server: copied to temp ${destination}`);
		return destination;
	} catch (error) {
		logExtension(`server: temp-copy failed: ${error}`);
		return source;
	}
}

function vsixPlatform(): string | null {
	const { arch } = process;
	switch (process.platform) {
		case "win32": {
			return arch === "arm64" ? "win32-arm64" : "win32-x64";
		}
		case "darwin": {
			return arch === "arm64" ? "darwin-arm64" : "darwin-x64";
		}
		case "linux": {
			return arch === "arm64" ? "linux-arm64" : "linux-x64";
		}
		default: {
			return null;
		}
	}
}

const WINDOWS_EXECUTABLE_EXTENSIONS = new Set(
	(process.env["PATHEXT"] ?? ".COM;.EXE;.BAT;.CMD")
		.split(";")
		.map((extension) => extension.trim().toLowerCase())
		.filter((extension) => extension.length > 0),
);

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

async function findOnPath(): Promise<string | null> {
	for (const directory of (process.env["PATH"] ?? "").split(path.delimiter)) {
		if (!directory) {
			continue;
		}
		const candidate = path.join(directory, BINARY_NAME);
		// oxlint-disable-next-line no-await-in-loop -- PATH order is a preference order; probe sequentially.
		if (await isExecutable(candidate)) {
			return candidate;
		}
	}
	return null;
}

export function downloadServer(extensionVersion: string | undefined): void {
	const platform = vsixPlatform();
	if (!platform) {
		void vscode.window.showErrorMessage(
			`Luck: no prebuilt binary for ${process.platform}/${process.arch}. Install via cargo and set luck.server.path.`,
		);
		return;
	}
	const url = `https://github.com/esoware/luck/releases/download/v${extensionVersion ?? "latest"}/luck-${platform}.zip`;
	void vscode.window.showInformationMessage(
		`Luck: download from ${url} not yet implemented. Set luck.server.path or install luck via cargo.`,
	);
}
