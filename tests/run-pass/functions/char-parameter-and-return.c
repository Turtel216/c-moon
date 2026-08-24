// A `char` crosses a call boundary in both directions: as an argument and as
// a returned value, each held in the low byte of the ABI register.
//@ exit-code: 90

char upper(char c) {
    if (c < 'a') {
        return c;
    }
    return c - 32;
}

int main() {
    char lower = 'z';
    return upper(lower);
}
