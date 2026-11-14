// Which dropped files belong where (without DOM – also testable in Node).

/** Extensions that Klangwelt inserts as audio (format detection in the core; MP3 & co. are rejected there with a reason). */
const AUDIO = /\.(wav|wave|bwf|aif|aiff|aifc|flac|opus|ogg|oga|mp3|m4a|aac|mp4|wma)$/i;
/** Scores for the library – and folders (without extension). */
const SCORE = /\.(mxl|musicxml|xml)$/i;

export const isAudioPath = (p: string): boolean => AUDIO.test(p);
export const isScorePath = (p: string): boolean => SCORE.test(p) || !/\.[a-z0-9]{1,8}$/i.test(p);

/** File name without folder. */
export const baseName = (p: string): string => p.split(/[\\/]/).pop() ?? p;
