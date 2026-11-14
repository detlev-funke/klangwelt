// Welche gezogenen Dateien wohin gehören (ohne DOM – auch in Node testbar).

/** Endungen, die Klangwelt als Audio einfügt (Formaterkennung im Kern; MP3 & Co. werden dort mit Grund abgelehnt). */
const AUDIO = /\.(wav|wave|bwf|aif|aiff|aifc|flac|opus|ogg|oga|mp3|m4a|aac|mp4|wma)$/i;
/** Partituren für die Bibliothek – und Ordner (ohne Endung). */
const SCORE = /\.(mxl|musicxml|xml)$/i;

export const isAudioPath = (p: string): boolean => AUDIO.test(p);
export const isScorePath = (p: string): boolean => SCORE.test(p) || !/\.[a-z0-9]{1,8}$/i.test(p);

/** Dateiname ohne Ordner. */
export const baseName = (p: string): string => p.split(/[\\/]/).pop() ?? p;
