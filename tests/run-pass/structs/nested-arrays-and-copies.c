// Everything at once: an array of structs inside a struct, reached through a
// pointer, copied element by element and as a whole, with the results driving
// control flow. The optimised and unoptimised pipelines and GCC must all agree.
//@ exit-code: 27

struct Vec { long int x; long int y; char tag; };
struct Grid { struct Vec cells[3]; int count; unsigned char flags; };

long int dot(struct Vec *a, struct Vec *b) {
    return a->x * b->x + a->y * b->y;
}

void fill(struct Grid *g) {
    int i;
    for (i = 0; i < 3; i = i + 1) {
        g->cells[i].x = i + 1;
        g->cells[i].y = (i + 1) * 2;
        g->cells[i].tag = 'x' + i;
    }
    g->count = 3;
    g->flags = 250;
}

int main() {
    struct Grid g;
    fill(&g);

    long int sum = 0;
    int i;
    for (i = 0; i < g.count; i = i + 1) {
        struct Vec v = g.cells[i];
        sum = sum + dot(&v, &g.cells[i]) + (v.tag - 'x');
        if (v.x > 1 && v.y < 100) {
            sum = sum + 1;
        }
    }

    struct Grid copy = g;
    copy.cells[0] = copy.cells[2];
    sum = sum + copy.cells[0].x + copy.flags - g.cells[0].x;

    return (int)sum - 300;
}
