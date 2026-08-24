// Elements of a `char[]` are one byte apart, so a write to one is invisible
// to its neighbours and indexing scales by one.
//@ exit-code: 162

int main() {
    char letters[4];
    letters[0] = 'a';
    letters[1] = 'b';
    letters[2] = 'c';
    letters[3] = 0;

    letters[1] = 'B';

    int sum = 0;
    int i = 0;
    while (letters[i]) {
        sum = sum + letters[i];
        i = i + 1;
    }
    // 'a' + 'B' + 'c', kept inside the byte an exit status is.
    return sum - 100;
}
