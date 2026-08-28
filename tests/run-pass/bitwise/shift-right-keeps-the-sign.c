// Shifting a signed value right copies its sign down, so a negative value
// stays negative however far it goes.
//@ exit-code: 7

int main() {
    int negative = -64;

    if ((negative >> 3) != -8) {
        return 1;
    }
    if ((negative >> 20) != -1) {
        return 2;
    }
    return 60 >> 3;
}
