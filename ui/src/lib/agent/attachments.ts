/**
 * Chat-pane attachments: classify dropped/picked files, inline text for the
 * agent, and package images/binaries for the ACP turn.
 *
 * The Aether ChatInput already accepts File[] (paperclip + its own drop).
 * This module is what actually *reads* them — previously agentSend only
 * showed "text-only for now" and discarded the bytes.
 */

export const ATTACHED_DOCS_HEADING = '# Attached documents';

export const MAX_FILES = 12;
export const MAX_FILE_BYTES = 8 * 1024 * 1024;
export const MAX_TEXT_INLINE = 400_000;
export const MAX_TOTAL_INLINE = 1_200_000;
export const MAX_IMAGE_BYTES_TOTAL = 16 * 1024 * 1024;

const TEXT_EXT = new Set([
	'md',
	'txt',
	'json',
	'xml',
	'svg',
	'csv',
	'tsv',
	'yml',
	'yaml',
	'toml',
	'veil',
	'layer',
	'stub',
	'mmd',
	'mermaid',
	'puml',
	'plantuml',
	'dot',
	'gv',
	'drawio',
	'excalidraw',
	'html',
	'htm',
	'css',
	'js',
	'ts',
	'mts',
	'cts',
	'rs',
	'sql',
	'graphql',
	'proto',
	'log',
	'ini',
	'cfg',
	'conf',
	'properties',
	'env',
	'sh',
	'bash',
	'py',
	'go',
	'java',
	'kt',
	'swift',
	'c',
	'h',
	'cpp',
	'hpp',
	'rb',
	'php',
	'r',
	'tex',
	'rst',
	'adoc',
	'org'
]);

export type AttachmentKind = 'text' | 'image' | 'pdf' | 'binary';

export interface IngestedAttachment {
	name: string;
	mime: string;
	kind: AttachmentKind;
	size: number;
	/** UTF-8 body when kind is text (possibly truncated). */
	text?: string;
	/** Raw base64 (no data: prefix) for images / pdf / binary. */
	dataBase64?: string;
	truncated?: boolean;
	skipped?: string;
}

export interface PreparedAttachments {
	/** Shown in the chat bubble (prompt + file names, not full bodies). */
	displayText: string;
	/** Last user message sent on the wire (includes inlined text files). */
	wireText: string;
	/** Extra payload for /api/agent/chat (images + binaries). */
	attachments: WireAttachment[];
	/** Object URLs for image previews in the bubble — caller must revoke. */
	previewUrls: { name: string; url: string; mime: string }[];
	warnings: string[];
}

export interface WireAttachment {
	name: string;
	mimeType: string;
	kind: AttachmentKind;
	text?: string;
	dataBase64?: string;
}

export function mergeFiles(a: File[], b: File[]): File[] {
	const out: File[] = [];
	const seen = new Set<string>();
	for (const f of [...a, ...b]) {
		const key = `${f.name}:${f.size}:${f.lastModified}`;
		if (seen.has(key)) continue;
		seen.add(key);
		out.push(f);
	}
	return out;
}

export function classifyFile(file: File): AttachmentKind {
	const mime = (file.type || '').toLowerCase();
	const ext = extOf(file.name);
	if (mime.startsWith('image/') && ext !== 'svg') return 'image';
	if (mime === 'application/pdf' || ext === 'pdf') return 'pdf';
	if (mime.startsWith('text/')) return 'text';
	if (
		mime === 'application/json' ||
		mime === 'application/xml' ||
		mime === 'text/xml' ||
		mime === 'image/svg+xml' ||
		mime === 'application/x-yaml' ||
		mime.endsWith('+xml') ||
		mime.endsWith('+json')
	) {
		return 'text';
	}
	if (TEXT_EXT.has(ext)) return 'text';
	return 'binary';
}

export async function prepareAttachments(
	prompt: string,
	files: File[]
): Promise<PreparedAttachments> {
	const warnings: string[] = [];
	let list = files.slice(0, MAX_FILES);
	if (files.length > MAX_FILES) {
		warnings.push(`Only the first ${MAX_FILES} files were attached (${files.length} dropped).`);
	}

	const ingested: IngestedAttachment[] = [];
	let inlineUsed = 0;
	let imageBytes = 0;

	for (const file of list) {
		const kind = classifyFile(file);
		const mime = file.type || guessMime(file.name, kind);
		if (file.size > MAX_FILE_BYTES) {
			ingested.push({
				name: file.name,
				mime,
				kind,
				size: file.size,
				skipped: `larger than ${Math.round(MAX_FILE_BYTES / (1024 * 1024))}MB`
			});
			warnings.push(`${file.name} skipped (too large).`);
			continue;
		}

		if (kind === 'text') {
			let text = await file.text();
			let truncated = false;
			const room = Math.max(0, MAX_TOTAL_INLINE - inlineUsed);
			const cap = Math.min(MAX_TEXT_INLINE, room);
			if (text.length > cap) {
				text = `${text.slice(0, cap)}\n\n…[truncated ${text.length - cap} chars]`;
				truncated = true;
				warnings.push(`${file.name} truncated to fit the prompt.`);
			}
			inlineUsed += text.length;
			ingested.push({ name: file.name, mime, kind, size: file.size, text, truncated });
			continue;
		}

		if (kind === 'image') {
			if (imageBytes + file.size > MAX_IMAGE_BYTES_TOTAL) {
				ingested.push({
					name: file.name,
					mime,
					kind,
					size: file.size,
					skipped: 'image budget exceeded'
				});
				warnings.push(`${file.name} skipped (image budget exceeded).`);
				continue;
			}
			imageBytes += file.size;
			const dataBase64 = await fileToBase64(file);
			ingested.push({ name: file.name, mime, kind, size: file.size, dataBase64 });
			continue;
		}

		// pdf / binary — send bytes so the host can persist + (for pdf) attach
		const dataBase64 = await fileToBase64(file);
		ingested.push({ name: file.name, mime, kind, size: file.size, dataBase64 });
	}

	const userLine =
		prompt.trim() ||
		(ingested.length
			? 'Please read the attached documents and use them as the source of truth for this request.'
			: '');

	const sections: string[] = [];
	if (ingested.length) {
		sections.push(
			`${ATTACHED_DOCS_HEADING}\nThe operator dropped ${ingested.length} file(s) into the agent chat. Read every attachment before answering. Diagrams / ERDs / layer maps are authoritative.`
		);
		for (const att of ingested) {
			const head = `## ${att.name} (${att.mime || att.kind}, ${att.size} bytes)`;
			if (att.skipped) {
				sections.push(`${head}\n[not inlined: ${att.skipped}]`);
			} else if (att.kind === 'text' && att.text != null) {
				const fence = fenceFor(att.name);
				sections.push(`${head}\n\`\`\`${fence}\n${att.text}\n\`\`\``);
			} else if (att.kind === 'image') {
				sections.push(
					`${head}\n[Raster image — also sent as a vision content block on this turn. Describe and use what you see.]`
				);
			} else {
				sections.push(
					`${head}\n[Binary attached. The host saves this under the turn attachment dir; read that file if you have a filesystem tool.]`
				);
			}
		}
	}

	const wireText = [userLine, ...sections].filter(Boolean).join('\n\n');
	const names = ingested.map((a) => a.name);
	const displayText = prompt.trim()
		? prompt.trim()
		: names.length
			? `Attached ${names.join(', ')}`
			: '';

	const previewUrls = ingested
		.filter((a) => a.kind === 'image' && !a.skipped)
		.map((a) => {
			const file = list.find((f) => f.name === a.name && f.size === a.size);
			return file
				? { name: a.name, url: URL.createObjectURL(file), mime: a.mime }
				: null;
		})
		.filter((x): x is { name: string; url: string; mime: string } => x != null);

	const attachments: WireAttachment[] = ingested
		.filter((a) => !a.skipped)
		.map((a) => ({
			name: a.name,
			mimeType: a.mime,
			kind: a.kind,
			text: a.kind === 'text' ? undefined : a.text,
			dataBase64: a.dataBase64
		}))
		.filter((a) => a.dataBase64 || a.text);

	return { displayText, wireText, attachments, previewUrls, warnings };
}

export function hasFileDrag(e: DragEvent): boolean {
	const types = e.dataTransfer?.types;
	if (!types) return false;
	return Array.from(types).includes('Files');
}

function extOf(name: string): string {
	const i = name.lastIndexOf('.');
	if (i < 0) return '';
	return name.slice(i + 1).toLowerCase();
}

function guessMime(name: string, kind: AttachmentKind): string {
	const ext = extOf(name);
	if (kind === 'image') {
		if (ext === 'png') return 'image/png';
		if (ext === 'jpg' || ext === 'jpeg') return 'image/jpeg';
		if (ext === 'gif') return 'image/gif';
		if (ext === 'webp') return 'image/webp';
		if (ext === 'avif') return 'image/avif';
		return 'image/png';
	}
	if (kind === 'pdf') return 'application/pdf';
	if (ext === 'svg') return 'image/svg+xml';
	if (ext === 'json' || ext === 'excalidraw') return 'application/json';
	if (ext === 'xml' || ext === 'drawio') return 'application/xml';
	if (kind === 'text') return 'text/plain';
	return 'application/octet-stream';
}

function fenceFor(name: string): string {
	const ext = extOf(name);
	if (ext === 'md' || ext === 'markdown') return 'markdown';
	if (ext === 'drawio' || ext === 'xml' || ext === 'svg') return 'xml';
	if (ext === 'excalidraw' || ext === 'json') return 'json';
	if (ext === 'yml' || ext === 'yaml') return 'yaml';
	if (ext === 'veil' || ext === 'layer' || ext === 'stub') return 'veil';
	if (ext === 'mmd' || ext === 'mermaid') return 'mermaid';
	if (ext === 'puml' || ext === 'plantuml') return 'text';
	return ext || 'text';
}

async function fileToBase64(file: File): Promise<string> {
	const buf = await file.arrayBuffer();
	const bytes = new Uint8Array(buf);
	let binary = '';
	const chunk = 0x8000;
	for (let i = 0; i < bytes.length; i += chunk) {
		binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
	}
	return btoa(binary);
}
