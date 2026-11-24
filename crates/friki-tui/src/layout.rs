//! Árbol de disposición de una pestaña: divisiones con proporción y paneles como hojas.
//! Lógica pura, sin E/S.

use ratatui::layout::Rect;

pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Línea de división horizontal: un panel encima del otro (`"` en tmux).
    Horizontal,
    /// Línea de división vertical: uno al lado del otro (`%` en tmux).
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf(PaneId),
    Split {
        dir: Dir,
        /// Parte del primer hijo, en milésimas.
        ratio: u16,
        a: Box<Node>,
        b: Box<Node>,
    },
}

const HALF: u16 = 500;

/// Tamaño del primer hijo: `total * ratio`, redondeado, dejando al menos 1 al segundo
/// si hay sitio. Con tamaño impar el primero se queda el sobrante (25 -> 13 + 12).
fn first_size(total: u16, ratio: u16) -> u16 {
    let first = ((u32::from(total) * u32::from(ratio) + 500) / 1000) as u16;
    first.min(total.saturating_sub(1)).max(total.min(1))
}

impl Node {
    pub fn leaves(&self) -> Vec<PaneId> {
        match self {
            Node::Leaf(id) => vec![*id],
            Node::Split { a, b, .. } => {
                let mut v = a.leaves();
                v.extend(b.leaves());
                v
            }
        }
    }

    /// Divide la hoja `target`: queda como primer hijo y `new` como segundo.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: Dir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    dir,
                    ratio: HALF,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, dir) || b.split(target, new, dir),
        }
    }

    /// Quita la hoja `id`; su hermano ocupa el sitio de la división. `None` si no queda nada.
    pub fn remove(self, id: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(l) => (l != id).then_some(Node::Leaf(l)),
            Node::Split { dir, ratio, a, b } => match ((*a).remove(id), (*b).remove(id)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (x, None) | (None, x) => x,
            },
        }
    }

    /// Reparte a partes iguales todas las divisiones.
    pub fn balance(&mut self) {
        if let Node::Split { ratio, a, b, .. } = self {
            *ratio = HALF;
            a.balance();
            b.balance();
        }
    }

    /// Áreas de las hojas y de los separadores verticales (1 columna) dentro de `area`.
    /// Las divisiones horizontales no gastan fila: el título del panel de abajo hace de línea.
    pub fn areas(&self, area: Rect, panes: &mut Vec<(PaneId, Rect)>, seps: &mut Vec<Rect>) {
        match self {
            Node::Leaf(id) => panes.push((*id, area)),
            Node::Split {
                dir: Dir::Horizontal,
                ratio,
                a,
                b,
            } => {
                let h1 = first_size(area.height, *ratio);
                a.areas(Rect { height: h1, ..area }, panes, seps);
                let r2 = Rect {
                    y: area.y + h1,
                    height: area.height - h1,
                    ..area
                };
                b.areas(r2, panes, seps);
            }
            Node::Split {
                dir: Dir::Vertical,
                ratio,
                a,
                b,
            } => {
                let avail = area.width.saturating_sub(1);
                let w1 = first_size(avail, *ratio);
                a.areas(Rect { width: w1, ..area }, panes, seps);
                if area.width > 0 {
                    seps.push(Rect {
                        x: area.x + w1,
                        width: 1,
                        ..area
                    });
                }
                let r2 = Rect {
                    x: area.x + w1 + 1,
                    width: avail - w1,
                    ..area
                };
                b.areas(r2, panes, seps);
            }
        }
    }
}

/// Hoja siguiente (`delta` = 1) o anterior (-1) a `cur`, con vuelta al principio/final.
pub fn cycle(leaves: &[PaneId], cur: PaneId, delta: isize) -> PaneId {
    let n = leaves.len() as isize;
    let i = leaves.iter().position(|&l| l == cur).unwrap_or(0) as isize;
    leaves
        .get((i + delta).rem_euclid(n.max(1)) as usize)
        .copied()
        .unwrap_or(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Node {
        // 1 | (2 / 3)
        let mut t = Node::Leaf(1);
        assert!(t.split(1, 2, Dir::Vertical));
        assert!(t.split(2, 3, Dir::Horizontal));
        t
    }

    #[test]
    fn split_and_leaves_in_order() {
        let t = tree();
        assert_eq!(t.leaves(), vec![1, 2, 3]);
        let mut t = t;
        assert!(!t.split(99, 4, Dir::Vertical));
    }

    #[test]
    fn remove_promotes_sibling_and_balances() {
        let mut t = tree();
        if let Node::Split { ratio, .. } = &mut t {
            *ratio = 800;
        }
        let mut t = t.remove(2).expect("quedan hojas");
        t.balance();
        assert_eq!(t.leaves(), vec![1, 3]);
        assert!(matches!(
            t,
            Node::Split {
                dir: Dir::Vertical,
                ratio: 500,
                ..
            }
        ));
        let t = t.remove(1).expect("queda 3");
        assert_eq!(t, Node::Leaf(3));
        assert!(t.remove(3).is_none());
    }

    #[test]
    fn areas_with_odd_sizes() {
        let mut panes = vec![];
        let mut seps = vec![];
        tree().areas(Rect::new(0, 0, 81, 25), &mut panes, &mut seps);
        // Ancho 81: 80 útiles + 1 separador -> 40 + 40.
        assert_eq!(panes[0], (1, Rect::new(0, 0, 40, 25)));
        assert_eq!(seps, vec![Rect::new(40, 0, 1, 25)]);
        // Alto 25 en dos: 13 arriba y 12 abajo.
        assert_eq!(panes[1], (2, Rect::new(41, 0, 40, 13)));
        assert_eq!(panes[2], (3, Rect::new(41, 13, 40, 12)));

        let (mut panes, mut seps) = (vec![], vec![]);
        tree().areas(Rect::new(0, 0, 80, 24), &mut panes, &mut seps);
        // 79 útiles: 40 + 39; todo el ancho queda cubierto.
        assert_eq!(panes[0].1.width + 1 + panes[1].1.width, 80);
        assert_eq!(panes[0].1.width, 40);
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 1)] {
            let (mut p, mut s) = (vec![], vec![]);
            tree().areas(Rect::new(0, 0, w, h), &mut p, &mut s);
            assert_eq!(p.len(), 3);
        }
    }

    #[test]
    fn focus_cycles() {
        let l = [1, 2, 3];
        assert_eq!(cycle(&l, 1, 1), 2);
        assert_eq!(cycle(&l, 3, 1), 1);
        assert_eq!(cycle(&l, 1, -1), 3);
        assert_eq!(cycle(&l, 2, -1), 1);
        assert_eq!(cycle(&[7], 7, 1), 7);
    }
}
