// A shift whose count changes on every iteration, building a mask a bit at a
// time and taking it apart again.
//@ exit-code: 31

int main() {
    int mask = 0;
    int i;

    for (i = 0; i < 5; i = i + 1) {
        mask = mask | (1 << i);
    }

    int counted = 0;
    while (mask != 0) {
        counted = counted + (mask & 1);
        mask = mask >> 1;
    }

    if (counted != 5) {
        return 1;
    }
    return 31;
}
