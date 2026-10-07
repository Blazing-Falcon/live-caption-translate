/** Native inputs toggle first; after the awaited save the DOM is set back to what the saved value says. */
export function reconcileChecked(input: HTMLInputElement, wanted: boolean): void {
  input.checked = wanted;
}

export function reconcileRadios(input: HTMLInputElement, wantedValue: string): void {
  const group = input.ownerDocument.querySelectorAll<HTMLInputElement>('input[type="radio"]');
  for (const radio of group) {
    if (radio.name === input.name) radio.checked = radio.value === wantedValue;
  }
}
