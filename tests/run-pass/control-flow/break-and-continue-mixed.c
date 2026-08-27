// Both jumps at once, across every kind of loop and operand this compiler
// has: an array scan that skips and then stops, nested loops each with their
// own, a short-circuiting condition around a `continue`, and an unsigned and
// a `long int` counter.
//@ exit-code: 65

struct Point { int x; int y; };

int classify(int v) {
    int n = 0;

    while (1) {
        n = n + 1;
        if (v < 0) {
            break;
        }
        if (n > v) {
            break;
        }
        continue;
    }

    return n;
}

int main() {
    int a[6];
    a[0] = -1; a[1] = 5; a[2] = 7; a[3] = 200; a[4] = 9; a[5] = 3;

    int total = 0;
    for (int i = 0; i < 6; i = i + 1) {
        if (a[i] < 0) {
            continue;
        }
        if (a[i] > 100) {
            break;
        }
        total = total + a[i];
    }

    total = total + classify(4);

    struct Point p;
    p.x = 0;
    p.y = 0;
    for (int i = 0; i < 4; i = i + 1) {
        for (int j = 0; j < 4; j = j + 1) {
            if (j > i) break;
            if (j == 0) continue;
            p.x = p.x + j;
        }
        if (p.x > 6) break;
        p.y = p.y + 1;
    }

    char c = 0;
    unsigned int u = 0;
    while (u < 20) {
        u = u + 1;
        if (u > 5 && u < 15) continue;
        c = c + 1;
    }

    long int k = 0;
    for (;;) {
        k = k + 3;
        if (k > 20) break;
        continue;
    }

    return total + p.x + p.y * 2 + c + (int)k;
}
