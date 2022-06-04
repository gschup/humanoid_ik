use crate::components::{ArmatureGraph, Bone, IkData, IkGoal, IkSettings};
use bevy::{
    prelude::*,
    utils::{HashMap, HashSet},
};
use std::collections::VecDeque;

pub fn create_armature_tree(
    bone_parents: Query<(Entity, &Children), With<Bone>>,
    bones: Query<Entity, With<Bone>>,
    mut graph: ResMut<ArmatureGraph>,
) {
    // clear the graph
    graph.out_bones.clear();
    graph.in_bone.clear();
    graph.base_joint.clear();

    let mut joint_id = 0;

    for (par_id, children) in bone_parents.iter() {
        let mut has_children = false;
        // gather all children bones - go through all children entities
        for &child_id in children.iter() {
            // if the child is a bone, register the bone as an out_bone of this next joint
            if let Ok(bone_id) = bones.get(child_id) {
                has_children = true;
                graph
                    .out_bones
                    .entry(joint_id)
                    .or_insert(HashSet::new())
                    .insert(bone_id);
            }
        }

        // register this bone as the incoming bone for this joint - but only if it had children
        // leaf bones should not have joints at their end
        if has_children {
            graph.in_bone.insert(joint_id, par_id);
        }

        // increment joint id for the next one
        joint_id += 1;
    }

    // add missing root joints and missing leaf bones
    // (they are not between two bones, so we didn't catch them earlier)
    for bone_id in bones.iter() {
        let mut is_out_bone = false;
        for out_bones in graph.out_bones.values() {
            if out_bones.contains(&bone_id) {
                is_out_bone = true;
            }
        }

        // if this bone is a root bone, add a root joint
        if !is_out_bone {
            graph
                .out_bones
                .entry(joint_id)
                .or_insert(HashSet::new())
                .insert(bone_id);
        }
    }

    // register base joint for each bone
    let mut base_joint = HashMap::<Entity, u32>::new();
    for (jid, out_bones) in graph.out_bones.iter() {
        for out_bone in out_bones {
            assert!(!graph.base_joint.contains_key(out_bone));
            base_joint.insert(*out_bone, *jid);
        }
    }
    graph.base_joint = base_joint;

    // joint - joint relations
    let mut joint_children = HashMap::<u32, HashSet<u32>>::new();
    let mut joint_parent = HashMap::<u32, u32>::new();
    for (par_id, out_bones) in graph.out_bones.iter() {
        for out_bone in out_bones.iter() {
            for (child_id, in_bone) in graph.in_bone.iter() {
                if out_bone == in_bone {
                    joint_parent.insert(*child_id, *par_id);
                    joint_children
                        .entry(*par_id)
                        .or_insert(HashSet::new())
                        .insert(*child_id);
                }
            }
        }
    }
    graph.joint_children = joint_children;
    graph.joint_parent = joint_parent;
}

pub fn cache_ik_data(
    bones: Query<(Entity, &Bone, &Transform, &GlobalTransform), With<Bone>>,
    goals: Query<(Entity, &GlobalTransform, &IkGoal), Without<Bone>>,
    graph: Res<ArmatureGraph>,
    mut data: ResMut<IkData>,
) {
    // reset data
    data.joint_positions.clear();
    data.required_positions.clear();
    data.joints_to_goals.clear();
    data.roots.clear();
    data.bone_lengths.clear();

    // register joint to goal mapping
    for (goal_id, _, goal) in goals.iter() {
        // register target joint from goal
        if let Some(base_joint) = graph.base_joint.get(&goal.target_bone) {
            data.joints_to_goals.insert(*base_joint, goal_id);
        }
    }

    // register roots and required positions
    for (_, _, goal) in goals.iter() {
        let mut cur_id = graph.base_joint.get(&goal.target_bone).unwrap();
        for i in 0..goal.chain_length {
            if let Some(par_id) = graph.joint_parent.get(cur_id) {
                // add the child bone as a required bone for the parent bone
                data.required_positions
                    .entry(*par_id)
                    .or_insert(HashSet::new())
                    .insert(*cur_id);
                cur_id = par_id;
            } else {
                // bone without parent, this is the root
                data.roots.insert(*cur_id);
                break;
            }

            //if we stop going up the tree due to chain length limitation, this node now also counts as a pseudo-root
            if i == goal.chain_length - 1 {
                data.roots.insert(*cur_id);
            }
        }
    }

    // initialize positions
    for (bone_id, _, _, gt) in bones.iter() {
        if let Some(base_joint) = graph.base_joint.get(&bone_id) {
            data.joint_positions.insert(*base_joint, gt.translation);
        }
    }
}

/*
pub fn compute_bone_positions(
    bones: Query<(Entity, &Bone, &Transform, &GlobalTransform), With<Bone>>,
    goals: Query<(Entity, &GlobalTransform, &IkGoal), Without<Bone>>,
    armature_graph: Res<ArmatureGraph>,
    settings: Res<IkSettings>,
    mut data: ResMut<IkData>,
) {
    // queue to walk through the armature graph
    let mut todo_queue = VecDeque::<Entity>::new();

    for _ in 0..settings.max_iterations {
        // check if target bones are close enough to the goals
        let mut highest_dist: f32 = 0.0;
        for (_, goal_tf, goal) in goals.iter() {
            let bone_tf = bones.get(goal.target_bone).unwrap().3;
            let dist = (goal_tf.translation - bone_tf.translation).length();
            highest_dist = highest_dist.max(dist);
        }
        if highest_dist < settings.goal_tolerance {
            break;
        }
        /*
         * FORWARD PASS - LEAF TO ROOT
         */

        // initialize todo queue with starting bones (bones with goals)
        todo_queue.clear(); // should be empty anyway, but just to make sure
        for (_, _, goal) in goals.iter() {
            todo_queue.push_back(goal.target_bone);
        }

        // new positions
        let mut new_positions = HashMap::<Entity, Vec3>::new();

        // actual forward pass
        while let Some(bone_id) = todo_queue.pop_front() {
            // check if all required bone children have a new position computed, otherwise push this bone back into the queue
            let mut ready = true;
            if let Some(reqs) = data.required_positions.get(&bone_id) {
                for req_id in reqs {
                    if !new_positions.contains_key(req_id) {
                        ready = false;
                        break;
                    }
                }
            }
            if !ready {
                todo_queue.push_back(bone_id);
                continue;
            }

            // figure out the new forward position for this bone
            if let Some(goal_id) = data.bones_to_goals.get(&bone_id) {
                // in the forward pass, the target bone of the goal is simply set to the goal position
                let goal_transform = goals.get(*goal_id).unwrap().1;
                new_positions.insert(bone_id, goal_transform.translation);
            } else {
                // otherwise compute a new position for each child
                // the new position is the centroid of those positions
                let old_pos = data.positions.get(&bone_id).unwrap();
                let children = data.required_positions.get(&bone_id).unwrap();
                let mut new_pos_centroid = Vec3::ZERO;
                for child_id in children {
                    let child_link_length = bones.get(*child_id).unwrap().2.translation.length();
                    let new_child_pos = new_positions.get(child_id).unwrap();
                    let dir = (*old_pos - *new_child_pos).normalize() * child_link_length;
                    let new_pos = *new_child_pos + dir;
                    new_pos_centroid += new_pos;
                }
                new_pos_centroid *= 1. / children.len() as f32;
                new_positions.insert(bone_id, new_pos_centroid);
            }

            // if we are not at the root, push the parent to the todo_queue, if it's not already in there
            if !data.roots.contains(&bone_id) {
                if let Some(par_id) = armature_graph.bone_parent.get(&bone_id) {
                    if !todo_queue.contains(par_id) {
                        todo_queue.push_back(*par_id);
                    }
                }
            }
        }

        /*
         * BACKWARD PASS - ROOT TO LEAF
         */

        // prepare todo queue for backward pass
        todo_queue.clear(); // should be empty anyway, but just to make sure
        for root in data.roots.iter() {
            todo_queue.push_back(*root);
        }

        // actual backward pass
        while let Some(bone_id) = todo_queue.pop_front() {
            // useful bone data
            let (_, _, transf, _) = bones.get(bone_id).unwrap();
            let link_length = transf.translation.length();
            // if this bone is one of the roots or pseudo-roots, we just set them back to their original position
            if data.roots.contains(&bone_id) {
                let old_pos = data.positions.get(&bone_id).unwrap();
                new_positions.insert(bone_id, *old_pos);
            } else {
                let par_id = armature_graph.bone_parent.get(&bone_id).unwrap();
                let par_pos = new_positions.get(par_id).unwrap();
                let forward_pos = new_positions.get(&bone_id).unwrap();
                let dir = (*forward_pos - *par_pos).normalize() * link_length;
                let backward_pos = *par_pos + dir;
                new_positions.insert(bone_id, backward_pos);
            }

            // put all required children in the todo queue - those who lead to a leaf bone with IK goal
            if let Some(children) = data.required_positions.get(&bone_id) {
                for child_id in children {
                    todo_queue.push_back(*child_id);
                }
            }
        }

        // "flip the buffer"
        data.positions = new_positions;
    }
}

pub fn apply_bone_positions(
    mut bone_transforms: Query<(&mut Transform, &GlobalTransform), With<Bone>>,
    armature_graph: Res<ArmatureGraph>,
    data: Res<IkData>,
) {
    // queue to walk through the armature graph
    let mut todo_queue = VecDeque::<Entity>::new();

    // updated - global transforms
    let mut global_transforms = HashMap::<Entity, Mat4>::new();

    // enqueue roots
    for root in data.roots.iter() {
        todo_queue.push_back(*root);
    }

    // apply position changes by rotation only - from root to children
    while let Some(bone_id) = todo_queue.pop_front() {
        // bone transforms
        let (local_tf, global_tf) = bone_transforms.get_mut(bone_id).unwrap();

        /*
        let pos = data.positions.get(&bone_id).unwrap();
        let par_pos = data.positions.get(par_id).unwrap();

        // compute global and local mat
        let global_mat = Transform::from_translation(*pos)
            .looking_at(*par_pos, Vec3::X)
            .compute_matrix();
        let local_mat = par_mat.inverse() * global_mat;

        // update local tf
        *bone_transforms.get_mut(bone_id).unwrap().0 = Transform::from_matrix(local_mat);
        */

        // enqueue relevant children
        if let Some(children) = data.required_positions.get(&bone_id) {
            for child_id in children {
                todo_queue.push_back(*child_id);
            }
        }
    }
}
*/
