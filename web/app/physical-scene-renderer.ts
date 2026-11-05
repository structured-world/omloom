import * as T from 'three';
import {OrbitControls} from 'three/addons/controls/OrbitControls.js';
import {AssetKind as K,key,type AssetRef} from './api';
import type {SceneBox,SceneLine} from './physical-scene-data';

/** Vue owns the lifecycle and accepted selection; GPU objects are reconstructible. */
export class PhysicalSceneRenderer {
  private renderer:T.WebGLRenderer;
  private scene=new T.Scene();
  private camera=new T.OrthographicCamera(-1,1,1,-1,.001,100000);
  private controls:OrbitControls;
  private observer:ResizeObserver;
  private frame:number|undefined;
  private meshes:T.Object3D[]=[];
  private selectable:T.Object3D[]=[];
  private radius=1;
  private center=new T.Vector3();
  private start:[number,number]|undefined;
  private lost=false;
  private disposed=false;
  constructor(private host:HTMLElement,private select:(target:AssetRef)=>void,private failure:(message:string)=>void){
    this.renderer=new T.WebGLRenderer({antialias:true,alpha:false});
    this.renderer.setClearColor('#f3f7f5');this.renderer.setPixelRatio(Math.min(devicePixelRatio,2));
    this.renderer.domElement.setAttribute('aria-label','Physical infrastructure 3D scene');
    this.renderer.domElement.setAttribute('role','img');
    host.append(this.renderer.domElement);
    this.controls=new OrbitControls(this.camera,this.renderer.domElement);this.controls.enableDamping=false;
    this.controls.addEventListener('change',this.invalidate);
    this.renderer.domElement.addEventListener('pointerdown',this.down);
    this.renderer.domElement.addEventListener('pointerup',this.up);
    this.renderer.domElement.addEventListener('webglcontextlost',this.contextLost);
    this.observer=new ResizeObserver(()=>this.resize());this.observer.observe(host);
  }
  private down=(e:PointerEvent)=>{this.start=[e.clientX,e.clientY];};
  private up=(e:PointerEvent)=>{
    if(!this.start||Math.hypot(e.clientX-this.start[0],e.clientY-this.start[1])>5)return;
    this.start=undefined;
    const rect=this.renderer.domElement.getBoundingClientRect(),ray=new T.Raycaster();
    ray.params.Line={threshold:this.radius*.008};
    ray.setFromCamera(new T.Vector2((e.clientX-rect.left)/rect.width*2-1,1-(e.clientY-rect.top)/rect.height*2),this.camera);
    const hit=ray.intersectObjects(this.selectable,false)[0];
    const target=hit?.object.userData.target as AssetRef|undefined;if(target)this.select(target);
  };
  private contextLost=(e:Event)=>{e.preventDefault();this.lost=true;if(this.frame!==undefined)cancelAnimationFrame(this.frame);this.frame=undefined;this.failure('The graphics context was lost. Use the resource list or retry the 3D view.');};
  private invalidate=()=>{if(this.frame!==undefined||this.lost||this.disposed)return;this.frame=requestAnimationFrame(()=>{this.frame=undefined;this.renderer.render(this.scene,this.camera);});};
  private resize(){const w=this.host.clientWidth,h=this.host.clientHeight;if(!w||!h)return;this.renderer.setSize(w,h);const aspect=w/h;this.camera.left=-this.radius*aspect;this.camera.right=this.radius*aspect;this.camera.top=this.radius;this.camera.bottom=-this.radius;this.camera.updateProjectionMatrix();this.invalidate();}
  private clear(){for(const mesh of this.meshes){this.scene.remove(mesh);const m=mesh as T.Mesh;m.geometry?.dispose();const materials=Array.isArray(m.material)?m.material:[m.material];for(const mat of materials)mat?.dispose();}this.meshes=[];this.selectable=[];}
  update(boxes:SceneBox[],lines:SceneLine[]){
    this.clear();const bounds=new T.Box3();
    for(const b of boxes){
      const geometry=new T.BoxGeometry(...b.size);
      const color=b.kind===K.PORT?'#347d8a':b.kind===K.RACK?'#53766b':'#a9c9c1';
      const mesh=new T.Mesh(geometry,new T.MeshBasicMaterial({color,transparent:b.root,opacity:b.root?.08:1,depthWrite:!b.root}));mesh.position.set(...b.center);mesh.userData.target=b.target;
      this.scene.add(mesh);this.meshes.push(mesh);if(!b.root)this.selectable.push(mesh);
      const edge=new T.LineSegments(new T.EdgesGeometry(geometry),new T.LineBasicMaterial({color:b.root?'#769c90':color}));edge.position.copy(mesh.position);edge.userData.target=b.target;this.scene.add(edge);this.meshes.push(edge);
      bounds.expandByObject(mesh);
    }
    for(const l of lines){const g=new T.BufferGeometry().setFromPoints([new T.Vector3(...l.from),new T.Vector3(...l.to)]);const line=new T.Line(g,new T.LineDashedMaterial({color:'#ad792e',dashSize:.03,gapSize:.015}));line.computeLineDistances();line.userData.target=l.target;this.scene.add(line);this.meshes.push(line);this.selectable.push(line);}
    if(!bounds.isEmpty()){bounds.getCenter(this.center);this.radius=Math.max(bounds.getSize(new T.Vector3()).length()*.65,.001);}
    this.camera.near=this.radius/10_000;this.camera.far=this.radius*10;
    this.reset();
  }
  highlight(target:AssetRef|undefined){for(const object of this.meshes){const m=object as T.Mesh;const material=m.material as T.MeshBasicMaterial|T.LineBasicMaterial;if(!material?.color)continue;const selected=target&&key(object.userData.target)===key(target);if(object.userData.baseColor===undefined)object.userData.baseColor=material.color.getHex();material.color.setHex(selected?0xb17926:object.userData.baseColor);}this.invalidate();}
  reset(){this.camera.position.copy(this.center).add(new T.Vector3(this.radius*1.4,this.radius,this.radius*1.6));this.camera.lookAt(this.center);this.controls.target.copy(this.center);this.controls.update();this.resize();}
  rotate(direction:number){const d=this.camera.position.clone().sub(this.controls.target).applyAxisAngle(new T.Vector3(0,1,0),direction*Math.PI/8);this.camera.position.copy(this.controls.target).add(d);this.camera.lookAt(this.controls.target);this.controls.update();this.invalidate();}
  dispose(){this.disposed=true;if(this.frame!==undefined)cancelAnimationFrame(this.frame);this.observer.disconnect();this.controls.removeEventListener('change',this.invalidate);this.controls.dispose();this.renderer.domElement.removeEventListener('pointerdown',this.down);this.renderer.domElement.removeEventListener('pointerup',this.up);this.renderer.domElement.removeEventListener('webglcontextlost',this.contextLost);this.clear();this.renderer.dispose();this.renderer.forceContextLoss();this.renderer.domElement.remove();}
}
