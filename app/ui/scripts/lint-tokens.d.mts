export interface TokenProblem {
  line: number;
  message: string;
}

export function lintSource(text: string): TokenProblem[];
export function lintTree(root: string): (TokenProblem & { file: string })[];
